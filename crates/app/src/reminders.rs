//! The Reminders on the home screen (#7): fiscal, legal and account notes in a
//! quiet corner, each dismissable until its interval runs out. Some show
//! only while raised, like the sales volume one (#28), checked again on
//! each Order Sync, the income one of affiliate Commissions (#77),
//! checked again whenever they change, the disclosure one of Offer Posts
//! (#102), checked again whenever a post is copied, the client secret
//! one of Amazon (#126), while its Connection holds the keys, the invoice
//! one of Amazon sales (#127), checked again on each Order Sync, and the
//! CNPJ one of the Shopee seller (#129), once any of its keys is saved.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Global, SharedString, Subscription, Window, div};
use mascate_integrations::{
    AMAZON_CLIENT_SECRET, Connection, ConnectionState, Connections, SHOPEE_SELLER_CNPJ,
};
use mascate_platform::{Reminder, ReminderError, ReminderTopic, Reminders};

use crate::affiliate::{self, CommissionChanges};
use crate::appearance::look;
use crate::connections::AppConnections;
use crate::finance::{self, VolumeSources};
use crate::kit;
use crate::offer_posts::{PostChanges, offer_posts};
use crate::orders::{self, OrderSyncs};

/// The app's Reminders; absent when the database did not open.
pub struct AppReminders(pub Arc<Reminders>);

impl Global for AppReminders {}

fn topic_name(topic: ReminderTopic) -> &'static str {
    match topic {
        ReminderTopic::Fiscal => "Fiscal",
        ReminderTopic::Legal => "Jurídico",
        ReminderTopic::Connection => "Conexão",
    }
}

/// The keys of the Connections' Reminders raised now: Amazon's client
/// secret while its keys are stored, expired ones included, and Shopee's
/// CNPJ once the owner starts setting up the seller Connection.
fn connection_reminders(connections: &Connections) -> Vec<&'static str> {
    let mut raised = Vec::new();
    if matches!(
        connections.state(Connection::Amazon),
        ConnectionState::Connected | ConnectionState::Expired
    ) {
        raised.push(AMAZON_CLIENT_SECRET.key);
    }
    let shopee_started = Connection::ShopeeSeller
        .credentials()
        .iter()
        .any(|credential| connections.has_credential(credential.name).unwrap_or(false));
    if shopee_started {
        raised.push(SHOPEE_SELLER_CNPJ.key);
    }
    raised
}

pub struct RemindersArea {
    showing: Vec<Reminder>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl RemindersArea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global::<OrderSyncs>(Self::refresh),
            cx.observe_global::<CommissionChanges>(Self::refresh),
            cx.observe_global::<PostChanges>(Self::refresh),
        ];
        let mut area = Self {
            showing: Vec::new(),
            reads: 0,
            error: None,
            _subscriptions: subscriptions,
        };
        area.refresh(cx);
        area
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.run(cx, |_| async { Ok(()) });
    }

    fn dismiss(&mut self, key: &'static str, cx: &mut Context<Self>) {
        // Gone at once; the read that follows confirms it.
        self.showing.retain(|reminder| reminder.key != key);
        self.run(
            cx,
            move |reminders| async move { reminders.dismiss(key).await },
        );
    }

    /// Runs a change off the UI thread, then reads what shows now.
    fn run<F, Fut>(&mut self, cx: &mut Context<Self>, change: F)
    where
        F: FnOnce(Arc<Reminders>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), ReminderError>> + Send,
    {
        let Some(app) = cx.try_global::<AppReminders>() else {
            return;
        };
        let reminders = app.0.clone();
        let sources = VolumeSources::of(cx);
        let commissions = affiliate::affiliate(cx);
        let posts = offer_posts(cx);
        let sales = orders::orders(cx);
        let connections = cx.try_global::<AppConnections>().map(|app| app.0.clone());
        self.reads += 1;
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let outcome = change(reminders.clone()).await;
            let mut raised = finance::raised_reminders(sources).await;
            raised.extend(affiliate::raised_reminders(commissions).await);
            if let Some(posts) = posts {
                // A failure raises nothing.
                match posts.raised_reminders().await {
                    Ok(keys) => raised.extend(keys),
                    Err(error) => eprintln!("could not check the posts for the Reminder: {error}"),
                }
            }
            if let Some(sales) = sales {
                match sales.raised_reminders().await {
                    Ok(keys) => raised.extend(keys),
                    Err(error) => eprintln!("could not check the Orders for the Reminder: {error}"),
                }
            }
            if let Some(connections) = connections {
                raised.extend(connection_reminders(&connections));
            }
            (outcome, reminders.showing(&raised).await)
        });
        cx.spawn(async move |this, cx| {
            let (outcome, showing) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                this.error = None;
                match showing {
                    Ok(showing) => this.showing = showing,
                    Err(error) => {
                        this.error = Some(format!("Não consegui ler os lembretes: {error}").into());
                    }
                }
                if let Err(error) = outcome {
                    this.error = Some(format!("Não consegui dispensar o lembrete: {error}").into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_reminder(&self, reminder: Reminder, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let days = reminder.reappears_after_days;
        h_flex()
            .gap_3()
            .items_start()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(kit::tag(topic_name(reminder.topic), t.text2, cx))
                            .child(div().font_medium().child(reminder.title)),
                    )
                    .child(div().text_color(t.text2).child(reminder.text)),
            )
            .child(
                Button::new(reminder.key)
                    .label("Dispensar")
                    .tooltip(format!("Volta em {days} dias."))
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.dismiss(reminder.key, cx))),
            )
            .into_any_element()
    }
}

impl Render for RemindersArea {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        if self.showing.is_empty() && self.error.is_none() {
            return div().into_any_element();
        }
        let items: Vec<AnyElement> = self
            .showing
            .clone()
            .into_iter()
            .map(|reminder| self.render_reminder(reminder, cx))
            .collect();
        v_flex()
            .gap_2()
            .child(div().text_sm().text_color(t.text2).child("Lembretes"))
            .when_some(self.error.clone(), |area, error| {
                area.child(kit::error_notice(error, cx))
            })
            .children(items)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mascate_platform::testing::MemorySecretStore;

    use super::*;

    #[test]
    fn shopees_cnpj_is_raised_once_any_seller_key_is_saved() {
        let connections = Connections::new(Arc::new(MemorySecretStore::default()));
        assert!(connection_reminders(&connections).is_empty());

        connections
            .save(
                Connection::ShopeeSeller,
                &[("SHOPEE_PARTNER_ID", "2001887")],
            )
            .unwrap();

        assert_eq!(connection_reminders(&connections), [SHOPEE_SELLER_CNPJ.key]);
    }
}
