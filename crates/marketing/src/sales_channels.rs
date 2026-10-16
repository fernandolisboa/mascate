//! Sales Channels in Marketing (#136, ADR 0033): every listing-keyed row
//! says which channel it belongs to, as Commerce's do since v12, and a
//! channel's own rating of the shop sits next to its Listing Quality.

use mascate_platform::Migration;

pub(crate) const ADD_SALES_CHANNELS: Migration = Migration {
    version: 11,
    name: "put every listing row on its Sales Channel",
    risky: false,
    // Adding a column with a default and renaming columns keep every row as
    // it was: what was there is Mercado Livre's.
    sql: "DROP INDEX marketing_listing_quality_by_item;
    ALTER TABLE marketing_listing_quality
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE marketing_listing_quality RENAME COLUMN ml_item_id TO channel_item_id;
    ALTER TABLE marketing_listing_quality ADD COLUMN unscored INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE marketing_listing_quality ADD COLUMN counts_visits INTEGER NOT NULL DEFAULT 1;
    CREATE UNIQUE INDEX marketing_listing_quality_by_item
        ON marketing_listing_quality (sales_channel, channel_item_id)
        WHERE deleted_at IS NULL;
    ALTER TABLE marketing_quality_actions RENAME COLUMN ml_rule_key TO channel_rule_key;

    DROP INDEX marketing_questions_by_question;
    ALTER TABLE marketing_questions
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE marketing_questions RENAME COLUMN ml_question_id TO channel_question_id;
    ALTER TABLE marketing_questions RENAME COLUMN ml_item_id TO channel_item_id;
    CREATE UNIQUE INDEX marketing_questions_by_question
        ON marketing_questions (sales_channel, channel_question_id)
        WHERE deleted_at IS NULL;

    DROP INDEX marketing_listing_reviews_by_item;
    ALTER TABLE marketing_listing_reviews
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE marketing_listing_reviews RENAME COLUMN ml_item_id TO channel_item_id;
    CREATE UNIQUE INDEX marketing_listing_reviews_by_item
        ON marketing_listing_reviews (sales_channel, channel_item_id)
        WHERE deleted_at IS NULL;

    DROP INDEX marketing_low_reviews_by_review;
    ALTER TABLE marketing_low_reviews
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE marketing_low_reviews RENAME COLUMN ml_review_id TO channel_review_id;
    ALTER TABLE marketing_low_reviews RENAME COLUMN ml_item_id TO channel_item_id;
    CREATE UNIQUE INDEX marketing_low_reviews_by_review
        ON marketing_low_reviews (sales_channel, channel_review_id)
        WHERE deleted_at IS NULL;

    DROP INDEX marketing_ad_campaigns_by_campaign;
    ALTER TABLE marketing_ad_campaigns
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE marketing_ad_campaigns RENAME COLUMN ml_campaign_id TO channel_campaign_id;
    CREATE UNIQUE INDEX marketing_ad_campaigns_by_campaign
        ON marketing_ad_campaigns (sales_channel, channel_campaign_id)
        WHERE deleted_at IS NULL;

    DROP INDEX marketing_ad_days_by_item_and_day;
    ALTER TABLE marketing_ad_days
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE marketing_ad_days RENAME COLUMN ml_item_id TO channel_item_id;
    ALTER TABLE marketing_ad_days RENAME COLUMN ml_campaign_id TO channel_campaign_id;
    CREATE UNIQUE INDEX marketing_ad_days_by_item_and_day
        ON marketing_ad_days (sales_channel, channel_item_id, day)
        WHERE deleted_at IS NULL;

    CREATE TABLE marketing_shop_performance (
        id            TEXT PRIMARY KEY,
        sales_channel TEXT NOT NULL,
        rating        TEXT,
        checked_at    TEXT NOT NULL,
        created_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL,
        deleted_at    TEXT
    );
    CREATE UNIQUE INDEX marketing_shop_performance_by_channel
        ON marketing_shop_performance (sales_channel)
        WHERE deleted_at IS NULL;
    CREATE TABLE marketing_performance_metrics (
        id                TEXT    PRIMARY KEY,
        performance_id    TEXT    NOT NULL REFERENCES marketing_shop_performance (id),
        channel_metric_id TEXT    NOT NULL,
        metric_group      TEXT    NOT NULL,
        name              TEXT    NOT NULL,
        current_value     TEXT,
        previous_value    TEXT,
        unit              TEXT    NOT NULL,
        target_value      TEXT,
        comparator        TEXT,
        position          INTEGER NOT NULL,
        created_at        TEXT    NOT NULL,
        updated_at        TEXT    NOT NULL,
        deleted_at        TEXT
    );
    CREATE UNIQUE INDEX marketing_performance_metrics_by_metric
        ON marketing_performance_metrics (performance_id, channel_metric_id)
        WHERE deleted_at IS NULL;",
};
