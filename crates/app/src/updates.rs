//! Updates (#8): a check for a newer version at start and every few hours,
//! the notice with its changelog and the "Atualizar" button, the silent
//! update the owner can turn on, and the way back to the version before
//! when a new one cannot migrate the database. The rules live in the
//! platform module.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Global, SharedString, Subscription, Window, div, px};
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_platform::{
    Database, Finish, Installation, RELEASES_PAGE, Release, ReleaseError, UpdateError,
    UpdateSettings, Updater, Version, run_installer_after_exit, save_update_settings,
};

use crate::appearance::look;
use crate::kit;
use crate::ordered_saves::OrderedSaves;

/// How often the app looks for a newer version while it runs.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// What the app is doing about updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Checking,
    Installing(Version),
    /// A silent update put this version in place for the next start.
    WaitingForRestart(Version),
    /// This version could not migrate the database: going back to the one
    /// before.
    GoingBack,
}

#[derive(Clone)]
pub struct AppUpdates {
    updater: Arc<Updater>,
    /// `None` when the database did not open: settings are not saved then.
    database: Option<Arc<Database>>,
    settings: UpdateSettings,
    available: Option<Release>,
    activity: Activity,
    checked_at: Option<DateTime<Local>>,
    /// Why the last check, install or way back failed.
    error: Option<String>,
    /// This version could not migrate the database at start.
    failed_to_migrate: bool,
    saves: Rc<RefCell<OrderedSaves>>,
}

impl Global for AppUpdates {}

/// Sets up updates for this run. Without a database (`None`), the owner
/// still sees newer versions, but silent updates stay off.
pub fn init(
    updater: Arc<Updater>,
    database: Option<Arc<Database>>,
    settings: UpdateSettings,
    failed_to_migrate: bool,
    cx: &mut App,
) {
    cx.set_global(AppUpdates {
        updater,
        database,
        settings,
        available: None,
        activity: Activity::Idle,
        checked_at: None,
        error: None,
        failed_to_migrate,
        saves: Default::default(),
    });
}

/// Checks now and every few hours after, for as long as the app runs.
pub fn start_checking(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.update(check_now);
            cx.background_executor().timer(CHECK_EVERY).await;
        }
    })
    .detach();
}

/// Looks for a newer version unless already busy; with silent updates on,
/// a version that may install silently is put in place for the next start.
pub fn check_now(cx: &mut App) {
    let app = cx.global_mut::<AppUpdates>();
    if app.activity != Activity::Idle {
        return;
    }
    app.activity = Activity::Checking;
    app.error = None;
    let updater = app.updater.clone();
    let checking = cx
        .background_executor()
        .spawn(async move { updater.check() });
    cx.spawn(async move |cx| {
        let checked = checking.await;
        cx.update(|cx| {
            let app = cx.global_mut::<AppUpdates>();
            app.activity = Activity::Idle;
            app.checked_at = Some(Local::now());
            match checked {
                Ok(release) => app.available = release,
                Err(error) => app.error = Some(check_failed(&error)),
            }
            install_silently_if_on(cx);
        });
    })
    .detach();
}

fn install_silently_if_on(cx: &mut App) {
    let app = cx.global_mut::<AppUpdates>();
    let Some(release) = app.available.clone() else {
        return;
    };
    if !app.settings.silent
        || app.activity != Activity::Idle
        || !app.updater.installs_silently(&release)
    {
        return;
    }
    app.activity = Activity::Installing(release.version);
    let updater = app.updater.clone();
    let installing = cx.background_executor().spawn(async move {
        updater
            .install_on_next_start(&release)
            .map(|()| release.version)
    });
    cx.spawn(async move |cx| {
        let installed = installing.await;
        cx.update(|cx| {
            let app = cx.global_mut::<AppUpdates>();
            match installed {
                Ok(version) => app.activity = Activity::WaitingForRestart(version),
                Err(error) => {
                    app.activity = Activity::Idle;
                    app.error = Some(install_failed(&error));
                }
            }
        });
    })
    .detach();
}

/// Installs the available version now: the app closes and the new one
/// opens.
fn install_now(cx: &mut App) {
    let app = cx.global_mut::<AppUpdates>();
    let Some(release) = app.available.clone() else {
        return;
    };
    if !matches!(
        app.activity,
        Activity::Idle | Activity::WaitingForRestart(_)
    ) {
        return;
    }
    app.activity = Activity::Installing(release.version);
    app.error = None;
    let updater = app.updater.clone();
    let installing = cx
        .background_executor()
        .spawn(async move { updater.install(&release) });
    finish_when_done(installing, cx);
}

/// Goes back to the version this one updated from, after it could not
/// migrate the database; the database is already as it was before.
pub fn go_back(cx: &mut App) {
    let app = cx.global_mut::<AppUpdates>();
    if app.activity != Activity::Idle {
        return;
    }
    app.activity = Activity::GoingBack;
    app.error = None;
    let updater = app.updater.clone();
    let going = cx
        .background_executor()
        .spawn(async move { updater.roll_back() });
    finish_when_done(going, cx);
}

/// Finishes an install when `task` has put the new version in place, or
/// says why it could not.
fn finish_when_done(task: gpui_kit::Task<Result<Finish, UpdateError>>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let done = task.await;
        cx.update(|cx| match done {
            Ok(finish) => {
                if let Err(error) = finish_install(finish, cx) {
                    let app = cx.global_mut::<AppUpdates>();
                    app.activity = Activity::Idle;
                    app.error = Some(error);
                }
            }
            Err(error) => {
                let app = cx.global_mut::<AppUpdates>();
                app.activity = Activity::Idle;
                app.error = Some(install_failed(&error));
            }
        });
    })
    .detach();
}

fn finish_install(finish: Finish, cx: &mut App) -> Result<(), String> {
    match finish {
        Finish::RunInstaller(installer) => {
            run_installer_after_exit(&installer)
                .map_err(|error| format!("Não consegui abrir o instalador: {error}"))?;
            cx.quit();
        }
        Finish::Restart(program) => {
            cx.set_restart_path(program);
            cx.restart();
        }
    }
    Ok(())
}

/// Restarts the app; an AppImage restarts from its file, since the folder
/// it runs from goes away when it closes.
pub fn restart(cx: &mut App) {
    if let Some(app) = cx.try_global::<AppUpdates>()
        && let Installation::AppImage(path) = app.updater.installation()
    {
        let path = path.clone();
        cx.set_restart_path(path);
    }
    cx.restart();
}

fn set_silent(silent: bool, cx: &mut App) {
    let app = cx.global_mut::<AppUpdates>();
    let Some(database) = app.database.clone() else {
        return;
    };
    let settings = UpdateSettings { silent };
    app.settings = settings;
    app.error = None;
    let saving = app.saves.clone().borrow_mut().save(
        async move {
            save_update_settings(&database, &SystemClock, &UuidV7Generator, settings)
                .await
                .map_err(|error| {
                    format!("Não consegui salvar a opção de atualização silenciosa: {error}")
                })
        },
        cx.background_executor(),
    );
    cx.spawn(async move |cx| {
        let saved = saving.await;
        cx.update(|cx| {
            if let Err(error) = saved {
                cx.global_mut::<AppUpdates>().error = Some(error);
            }
            install_silently_if_on(cx);
        });
    })
    .detach();
}

fn check_failed(error: &UpdateError) -> String {
    match error {
        UpdateError::Release(ReleaseError::RateLimited) => {
            "O GitHub pediu um tempo antes de procurar de novo; tento mais tarde.".into()
        }
        UpdateError::Release(ReleaseError::Network(_)) => {
            "Não consegui falar com o GitHub agora; tento mais tarde.".into()
        }
        error => format!("Não consegui procurar versão nova: {error}"),
    }
}

fn install_failed(error: &UpdateError) -> String {
    match error {
        UpdateError::NothingToGoBackTo => {
            "Não sei de qual versão o app veio, então não dá para voltar sozinho. Instale a \
             versão anterior pela página de versões no GitHub."
                .into()
        }
        UpdateError::NotPublished(version) => format!(
            "A versão {version} não está publicada no GitHub; instale outra pela página de \
             versões."
        ),
        UpdateError::Release(ReleaseError::Network(_) | ReleaseError::RateLimited) => {
            "Não consegui falar com o GitHub agora; tente de novo daqui a pouco.".into()
        }
        error => format!("Não consegui instalar: {error}"),
    }
}

fn installation_name(installation: &Installation) -> &'static str {
    match installation {
        Installation::WindowsInstaller => "instalada pelo instalador do Windows",
        Installation::AppImage(_) => "AppImage",
        Installation::Manual => "pacote do Linux ou versão de desenvolvimento",
    }
}

/// A newer version, a silent update waiting for a restart, or the way back
/// after a failed one, at the top of the home screen.
pub struct UpdateNotice {
    /// The owner asked to install a version with a risky migration and has
    /// not confirmed yet.
    confirming: bool,
    _observing: Subscription,
}

impl UpdateNotice {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            confirming: false,
            _observing: cx.observe_global::<AppUpdates>(|_, cx| cx.notify()),
        }
    }

    fn update(&mut self, risky: bool, cx: &mut Context<Self>) {
        if risky && !self.confirming {
            self.confirming = true;
            cx.notify();
            return;
        }
        self.confirming = false;
        install_now(cx);
    }

    fn render_available(
        &self,
        app: &AppUpdates,
        release: &Release,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let risky = app.updater.is_risky(release);
        let installable =
            app.updater.installation() != &Installation::Manual && release.installer.is_some();
        let installing = app.activity == Activity::Installing(release.version);
        let page = release.page.clone();
        let mut notice = v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent_edge)
            .bg(t.surface)
            .child(div().font_medium().child(format!(
                "A versão {} do Mascate está disponível (você tem a {}).",
                release.version,
                app.updater.current()
            )));
        if app.updater.failed_before(release) {
            notice = notice.child(kit::error_notice(
                format!(
                    "A {} já não conseguiu migrar o seu banco aqui; o app voltou para esta \
                     versão e o banco ficou como estava.",
                    release.version
                ),
                cx,
            ));
        }
        if let Activity::WaitingForRestart(version) = app.activity {
            notice = notice.child(div().text_sm().text_color(t.text2).child(format!(
                "A {version} já foi baixada e entra na próxima vez que o app abrir."
            )));
        }
        if !release.notes.trim().is_empty() {
            notice = notice.child(div().max_h(px(220.)).overflow_hidden().text_sm().child(
                TextView::markdown(
                    SharedString::from(format!("release-notes-{}", release.version)),
                    release.notes.clone(),
                ),
            ));
        }
        if self.confirming {
            notice = notice.child(div().text_sm().text_color(t.danger).child(
                "Esta versão mexe em dados que já estão no banco (migração de risco). \
                         O app faz um Backup antes; se a migração falhar, o banco volta como \
                         estava e o app volta para a versão atual.",
            ));
        } else if risky {
            notice = notice.child(div().text_sm().text_color(t.text2).child(
                "Traz uma migração de risco: nunca entra sozinha, só quando você confirmar.",
            ));
        }
        let mut buttons = h_flex().gap_2().flex_wrap();
        if installable {
            let label = match (self.confirming, &app.activity) {
                (true, _) => "Atualizar mesmo assim",
                (false, Activity::WaitingForRestart(_)) => "Reiniciar e atualizar",
                _ => "Atualizar",
            };
            let button = Button::new("install-update")
                .label(label)
                .small()
                .loading(installing)
                .disabled(!matches!(
                    app.activity,
                    Activity::Idle | Activity::WaitingForRestart(_)
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.update(risky, cx)));
            buttons = buttons.child(if self.confirming {
                button.danger()
            } else {
                button.primary()
            });
            if self.confirming {
                buttons = buttons.child(
                    Button::new("cancel-update")
                        .label("Agora não")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirming = false;
                            cx.notify();
                        })),
                );
            }
        } else {
            notice = notice.child(div().text_sm().text_color(t.text2).child(
                "Esta cópia do app se atualiza pelo pacote ou baixando a versão nova na página.",
            ));
        }
        buttons = buttons.child(
            Button::new("open-release")
                .label("Ver a versão no GitHub")
                .ghost()
                .small()
                .on_click(move |_, _, cx| cx.open_url(&page)),
        );
        notice
            .child(buttons)
            .when_some(app.error.clone(), |notice, error| {
                notice.child(kit::error_notice(error, cx))
            })
            .into_any_element()
    }

    fn render_going_back(&self, app: &AppUpdates, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let current = app.updater.current();
        let previous = app
            .updater
            .previous()
            .filter(|previous| *previous < current);
        let going = app.activity == Activity::GoingBack;
        let next = match (previous, going) {
            (Some(previous), true) => format!(" Voltando para a {previous}…"),
            (Some(previous), false) => format!(" Dá para voltar para a {previous}."),
            (None, _) => String::new(),
        };
        let mut buttons = h_flex().gap_2().flex_wrap();
        if let Some(previous) = previous {
            buttons = buttons.child(
                Button::new("go-back")
                    .label(format!("Voltar para a {previous}"))
                    .primary()
                    .small()
                    .loading(going)
                    .disabled(going)
                    .on_click(|_, _, cx| go_back(cx)),
            );
        }
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.danger)
            .child(div().font_medium().child(format!(
                "Esta versão ({current}) não conseguiu migrar o banco."
            )))
            .child(div().text_sm().text_color(t.text2).child(format!(
                "O banco voltou exatamente como estava, a partir do Backup feito antes da \
                 migração.{next}"
            )))
            .when_some(app.error.clone(), |notice, error| {
                notice.child(kit::error_notice(error, cx))
            })
            .child(
                buttons.child(
                    Button::new("open-releases")
                        .label("Ver as versões no GitHub")
                        .ghost()
                        .small()
                        .on_click(|_, _, cx| cx.open_url(RELEASES_PAGE)),
                ),
            )
            .into_any_element()
    }
}

impl Render for UpdateNotice {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = cx.try_global::<AppUpdates>().cloned() else {
            return div().into_any_element();
        };
        if app.failed_to_migrate {
            return self.render_going_back(&app, cx);
        }
        match app.available.clone() {
            Some(release) => self.render_available(&app, &release, cx),
            None => div().into_any_element(),
        }
    }
}

/// Settings › Atualizações: the version running, silent updates, and a
/// check on demand.
pub struct UpdatesSection {
    _observing: Subscription,
}

impl UpdatesSection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            _observing: cx.observe_global::<AppUpdates>(|_, cx| cx.notify()),
        }
    }
}

impl Render for UpdatesSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let Some(app) = cx.try_global::<AppUpdates>().cloned() else {
            return v_flex();
        };
        let installation = app.updater.installation();
        let manual = installation == &Installation::Manual;
        let checking = app.activity == Activity::Checking;
        let status = match (&app.available, app.checked_at) {
            (Some(release), _) => format!(
                "A versão {} está disponível; o aviso fica na tela Hoje.",
                release.version
            ),
            (None, Some(at)) => format!(
                "Procurei às {}: você já tem a versão mais nova.",
                at.format("%H:%M")
            ),
            (None, None) => "Ainda não procurei versão nova nesta sessão.".into(),
        };
        let silent_hint = if manual {
            "Esta cópia se atualiza pelo pacote do Linux ou é uma versão de desenvolvimento, \
             então não instala versões sozinha."
        } else {
            "Baixa a versão nova em segundo plano e instala na próxima vez que o app abrir. \
             Versões com migração de risco sempre esperam você confirmar."
        };
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(t.text2).child(format!(
                "Versão {} ({}). O app procura versão nova nas releases do GitHub ao abrir e \
                 a cada 6 horas, e faz um Backup antes de qualquer migração do banco.",
                app.updater.current(),
                installation_name(installation)
            )))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Checkbox::new("silent-updates")
                            .label("Atualizar silenciosamente")
                            .checked(app.settings.silent)
                            .disabled(manual || app.database.is_none())
                            .on_click(|checked, _, cx| set_silent(*checked, cx)),
                    )
                    .child(
                        div()
                            .pl(px(24.))
                            .text_xs()
                            .text_color(t.text2)
                            .child(silent_hint),
                    ),
            )
            .child(div().text_sm().text_color(t.text2).child(status))
            .when_some(app.error.clone(), |section, error| {
                section.child(kit::error_notice(error, cx))
            })
            .child(
                h_flex().child(
                    Button::new("check-updates")
                        .label("Procurar versão nova")
                        .outline()
                        .small()
                        .loading(checking)
                        .disabled(app.activity != Activity::Idle)
                        .on_click(|_, _, cx| check_now(cx)),
                ),
            )
    }
}
