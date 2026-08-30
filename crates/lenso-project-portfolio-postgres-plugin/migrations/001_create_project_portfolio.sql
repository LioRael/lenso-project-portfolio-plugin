CREATE TABLE portfolio_initiatives (
    organization_id text NOT NULL,
    initiative_id text NOT NULL,
    name text NOT NULL,
    summary text,
    owner_subject text,
    target_start date,
    target_date date,
    health text NOT NULL DEFAULT 'no_update',
    progress smallint NOT NULL DEFAULT 0,
    archived boolean NOT NULL DEFAULT false,
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    archived_at timestamptz,
    row_seq bigserial NOT NULL UNIQUE,
    PRIMARY KEY (organization_id, initiative_id),
    CHECK (health IN ('on_track', 'at_risk', 'off_track', 'no_update')),
    CHECK (progress BETWEEN 0 AND 100),
    CHECK (target_start IS NULL OR target_date IS NULL OR target_start <= target_date)
);

CREATE INDEX portfolio_initiatives_list_idx
    ON portfolio_initiatives (organization_id, archived, row_seq);

CREATE TABLE portfolio_projects (
    organization_id text NOT NULL,
    initiative_id text NOT NULL,
    project_id text NOT NULL,
    name_snapshot text NOT NULL,
    status_category text NOT NULL,
    health text NOT NULL,
    progress smallint NOT NULL,
    target_start date,
    target_date date,
    snapshot_revision text NOT NULL,
    observed_at timestamptz NOT NULL,
    position integer NOT NULL CHECK (position >= 0),
    membership_revision bigint NOT NULL DEFAULT 1 CHECK (membership_revision > 0),
    PRIMARY KEY (organization_id, initiative_id, project_id),
    UNIQUE (organization_id, initiative_id, position),
    FOREIGN KEY (organization_id, initiative_id)
        REFERENCES portfolio_initiatives (organization_id, initiative_id) ON DELETE CASCADE,
    CHECK (status_category IN ('backlog', 'planned', 'started', 'paused', 'completed', 'canceled')),
    CHECK (health IN ('on_track', 'at_risk', 'off_track', 'no_update')),
    CHECK (progress BETWEEN 0 AND 100),
    CHECK (target_start IS NULL OR target_date IS NULL OR target_start <= target_date)
);

CREATE INDEX portfolio_projects_order_idx
    ON portfolio_projects (organization_id, initiative_id, position, project_id);

CREATE TABLE portfolio_updates (
    organization_id text NOT NULL,
    initiative_id text NOT NULL,
    update_id text NOT NULL,
    health text NOT NULL,
    summary text NOT NULL,
    progress smallint NOT NULL,
    created_by text NOT NULL,
    initiative_revision bigint NOT NULL CHECK (initiative_revision > 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    row_seq bigserial NOT NULL UNIQUE,
    PRIMARY KEY (organization_id, update_id),
    FOREIGN KEY (organization_id, initiative_id)
        REFERENCES portfolio_initiatives (organization_id, initiative_id) ON DELETE CASCADE,
    CHECK (health IN ('on_track', 'at_risk', 'off_track', 'no_update')),
    CHECK (progress BETWEEN 0 AND 100)
);

CREATE INDEX portfolio_updates_list_idx
    ON portfolio_updates (organization_id, initiative_id, row_seq);

CREATE TABLE portfolio_commands (
    caller_instance text NOT NULL,
    actor_subject text NOT NULL,
    operation text NOT NULL,
    idempotency_key text NOT NULL,
    request_hash bytea NOT NULL,
    response_json jsonb,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (caller_instance, actor_subject, operation, idempotency_key)
);
