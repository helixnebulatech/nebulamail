// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! Diesel schema, kept by hand in sync with `migrations/` (the tests run the
//! migrations and every query, so a mismatch fails there).

diesel::table! {
    accounts (id) {
        id -> BigInt,
        login -> Text,
        email -> Text,
        wrapped_dek -> Binary,
        created_at -> BigInt,
        last_login_at -> BigInt,
    }
}

diesel::table! {
    account_secrets (account_id, field) {
        account_id -> BigInt,
        field -> Integer,
        sealed -> Binary,
    }
}

diesel::table! {
    oauth_codes (code_hash) {
        code_hash -> Binary,
        account_id -> BigInt,
        client_id -> Text,
        redirect_uri -> Text,
        code_challenge -> Text,
        scope -> Text,
        expires_at -> BigInt,
        used_at -> Nullable<BigInt>,
        reused -> Integer,
        grant_id -> Nullable<BigInt>,
    }
}

diesel::table! {
    oauth_grants (id) {
        id -> BigInt,
        account_id -> BigInt,
        client_id -> Text,
        scope -> Text,
        created_at -> BigInt,
        expires_at -> BigInt,
        revoked_at -> Nullable<BigInt>,
    }
}

diesel::table! {
    login_devices (token_hash, account_id) {
        token_hash -> Binary,
        account_id -> BigInt,
        last_login_at -> BigInt,
        expires_at -> BigInt,
    }
}

diesel::table! {
    oauth_tokens (token_hash) {
        token_hash -> Binary,
        grant_id -> BigInt,
        kind -> Integer,
        expires_at -> BigInt,
        used_at -> Nullable<BigInt>,
    }
}

diesel::table! {
    states (account_id, data_type) {
        account_id -> BigInt,
        data_type -> Integer,
        seq -> BigInt,
        oldest -> BigInt,
        log_rows -> BigInt,
    }
}

diesel::table! {
    changes (id) {
        id -> BigInt,
        account_id -> BigInt,
        data_type -> Integer,
        seq -> BigInt,
        object_id -> BigInt,
        kind -> Integer,
    }
}

diesel::table! {
    mailboxes (id) {
        id -> BigInt,
        account_id -> BigInt,
        imap_name -> Text,
        name -> Text,
        delimiter -> Nullable<Text>,
        parent_id -> Nullable<BigInt>,
        role -> Nullable<Integer>,
        subscribed -> Bool,
        selectable -> Bool,
        sort_order -> BigInt,
        uid_validity -> Nullable<BigInt>,
        uid_next -> Nullable<BigInt>,
        highest_modseq -> Nullable<BigInt>,
        messages -> Nullable<BigInt>,
        synced_at -> Nullable<BigInt>,
        no_inferiors -> Bool,
    }
}

diesel::table! {
    threads (id) {
        id -> BigInt,
        account_id -> BigInt,
        server_id -> Nullable<Text>,
    }
}

diesel::table! {
    emails (id) {
        id -> BigInt,
        account_id -> BigInt,
        thread_id -> BigInt,
        server_id -> Nullable<Text>,
        message_id -> Nullable<Text>,
        in_reply_to -> Text,
        references_ids -> Text,
        subject -> Nullable<Text>,
        from_addresses -> Nullable<Text>,
        to_addresses -> Nullable<Text>,
        cc_addresses -> Nullable<Text>,
        bcc_addresses -> Nullable<Text>,
        reply_to_addresses -> Nullable<Text>,
        sender_addresses -> Nullable<Text>,
        sent_at -> Nullable<BigInt>,
        sent_offset -> Nullable<Integer>,
        received_at -> BigInt,
        size -> BigInt,
        has_attachment -> Bool,
        preview -> Nullable<Text>,
        headers -> Binary,
        sort_subject -> Text,
        sort_from -> Text,
        orphaned_at -> Nullable<BigInt>,
    }
}

diesel::table! {
    email_locations (mailbox_id, uid) {
        mailbox_id -> BigInt,
        uid -> BigInt,
        email_id -> BigInt,
        modseq -> Nullable<BigInt>,
    }
}

diesel::table! {
    email_keywords (email_id, keyword) {
        email_id -> BigInt,
        keyword -> Text,
    }
}

diesel::table! {
    thread_refs (account_id, message_id) {
        account_id -> BigInt,
        message_id -> Text,
        thread_id -> BigInt,
    }
}

diesel::table! {
    uploads (id) {
        id -> BigInt,
        account_id -> BigInt,
        media_type -> Text,
        size -> BigInt,
        complete -> Bool,
        created_at -> BigInt,
        expires_at -> BigInt,
    }
}

diesel::table! {
    upload_chunks (upload_id, seq) {
        upload_id -> BigInt,
        seq -> BigInt,
        data -> Binary,
    }
}

diesel::table! {
    identities (id) {
        id -> BigInt,
        account_id -> BigInt,
        name -> Text,
        email -> Text,
        reply_to -> Nullable<Text>,
        bcc -> Nullable<Text>,
        text_signature -> Text,
        html_signature -> Text,
        may_delete -> Bool,
    }
}

diesel::table! {
    email_submissions (id) {
        id -> BigInt,
        account_id -> BigInt,
        identity_id -> BigInt,
        email_id -> BigInt,
        thread_id -> BigInt,
        envelope -> Text,
        send_at -> BigInt,
        smtp_reply -> Text,
        expires_at -> BigInt,
        message_id -> Nullable<Text>,
        envid -> Nullable<Text>,
        delivery -> Nullable<Text>,
        dsn_emails -> Text,
        mdn_emails -> Text,
        undo_status -> Text,
    }
}

diesel::table! {
    held_submissions (submission_id) {
        submission_id -> BigInt,
        account_id -> BigInt,
        message -> Binary,
        size -> BigInt,
        next_attempt_at -> BigInt,
        attempts -> BigInt,
        sent_copy -> Bool,
        send_when_signed_out -> Bool,
    }
}

diesel::table! {
    dav_accounts (account_id) {
        account_id -> BigInt,
        principal -> Text,
        calendar_homes -> Text,
        addressbook_homes -> Text,
        schedule_outbox -> Nullable<Text>,
        user_addresses -> Text,
        auto_schedule -> Bool,
        discovered_at -> BigInt,
    }
}

diesel::table! {
    dav_collections (id) {
        id -> BigInt,
        account_id -> BigInt,
        kind -> Integer,
        href -> Text,
        name -> Nullable<Text>,
        color -> Nullable<Text>,
        description -> Nullable<Text>,
        sort_order -> Nullable<BigInt>,
        time_zone -> Nullable<Text>,
        rights -> Nullable<Integer>,
        components -> Nullable<Integer>,
        vcard_versions -> Nullable<Integer>,
        supports_sync -> Bool,
        sync_token -> Nullable<Text>,
        ctag -> Nullable<Text>,
        synced_at -> Nullable<BigInt>,
        orphaned_at -> Nullable<BigInt>,
        is_visible -> Bool,
        is_subscribed -> Bool,
        is_default -> Bool,
        owner -> Nullable<Text>,
        share_mode -> Nullable<Integer>,
        shares -> Nullable<Text>,
        default_alarms_datetime -> Nullable<Text>,
        default_alarms_date -> Nullable<Text>,
        publishable -> Bool,
        is_subscription -> Bool,
        publish_url_sealed -> Nullable<Binary>,
        subscription_source_sealed -> Nullable<Binary>,
    }
}

diesel::table! {
    dav_resources (id) {
        id -> BigInt,
        account_id -> BigInt,
        collection_id -> BigInt,
        href -> Text,
        etag -> Nullable<Text>,
        uid -> Nullable<Text>,
        data -> Binary,
        size -> BigInt,
        utc_start -> Nullable<BigInt>,
        utc_end -> Nullable<BigInt>,
        recurring -> Bool,
        sort_name -> Nullable<Text>,
        invalid -> Bool,
        updated_at -> BigInt,
        orphaned_at -> Nullable<BigInt>,
    }
}

diesel::table! {
    calendar_alerts_fired (id) {
        id -> BigInt,
        account_id -> BigInt,
        resource_id -> BigInt,
        alert_id -> Text,
        occurrence -> BigInt,
        fired_at -> BigInt,
        repetition -> BigInt,
    }
}

diesel::table! {
    push_subscriptions (id) {
        id -> BigInt,
        account_id -> BigInt,
        grant_id -> BigInt,
        device_client_id -> Text,
        sealed -> Nullable<Binary>,
        types -> Nullable<Text>,
        expires -> BigInt,
        verification_code -> Text,
        verified -> Integer,
        created_at -> BigInt,
    }
}

diesel::table! {
    sieve_scripts (id) {
        id -> BigInt,
        account_id -> BigInt,
        name -> Text,
        active -> Integer,
        content_hash -> Text,
    }
}

diesel::table! {
    system_secrets (name) {
        name -> Text,
        wrapped_dek -> Binary,
        sealed -> Binary,
        created_at -> BigInt,
    }
}

diesel::joinable!(push_subscriptions -> accounts (account_id));
diesel::joinable!(push_subscriptions -> oauth_grants (grant_id));
diesel::joinable!(sieve_scripts -> accounts (account_id));
diesel::joinable!(account_secrets -> accounts (account_id));
diesel::joinable!(oauth_codes -> accounts (account_id));
diesel::joinable!(oauth_grants -> accounts (account_id));
diesel::joinable!(login_devices -> accounts (account_id));
diesel::joinable!(oauth_tokens -> oauth_grants (grant_id));
diesel::joinable!(states -> accounts (account_id));
diesel::joinable!(changes -> accounts (account_id));
diesel::joinable!(threads -> accounts (account_id));
diesel::joinable!(emails -> threads (thread_id));
diesel::joinable!(email_locations -> emails (email_id));
diesel::joinable!(email_locations -> mailboxes (mailbox_id));
diesel::joinable!(email_keywords -> emails (email_id));
diesel::joinable!(thread_refs -> threads (thread_id));
diesel::joinable!(uploads -> accounts (account_id));
diesel::joinable!(upload_chunks -> uploads (upload_id));
diesel::joinable!(identities -> accounts (account_id));
diesel::joinable!(email_submissions -> accounts (account_id));
diesel::joinable!(held_submissions -> email_submissions (submission_id));
diesel::joinable!(dav_accounts -> accounts (account_id));
diesel::joinable!(dav_collections -> accounts (account_id));
diesel::joinable!(dav_resources -> dav_collections (collection_id));
diesel::joinable!(calendar_alerts_fired -> dav_resources (resource_id));

diesel::allow_tables_to_appear_in_same_query!(
    push_subscriptions,
    sieve_scripts,
    dav_accounts,
    dav_collections,
    dav_resources,
    calendar_alerts_fired,
    accounts,
    account_secrets,
    login_devices,
    oauth_codes,
    oauth_grants,
    oauth_tokens,
    states,
    changes,
    mailboxes,
    threads,
    emails,
    email_locations,
    email_keywords,
    thread_refs,
    uploads,
    upload_chunks,
    identities,
    email_submissions,
    held_submissions,
);
