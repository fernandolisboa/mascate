//! Commerce: Listings, Orders, Purchase Orders and shipping on each Sales Channel.

use mascate_platform::{Reminder, ReminderTopic};

pub const BUYER_PERSONAL_DATA: Reminder = Reminder {
    key: "commerce.buyer_personal_data",
    topic: ReminderTopic::Legal,
    title: "Dados de compradores (LGPD)",
    text: "Nome, endereço e telefone de quem compra são dados pessoais. Use-os só para envio \
           e suporte; o app guarda apenas o necessário para isso.",
    reappears_after_days: 90,
};

/// This module's Reminders.
pub const REMINDERS: &[Reminder] = &[BUYER_PERSONAL_DATA];
