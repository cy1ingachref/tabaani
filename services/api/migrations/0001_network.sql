-- Tabaani schema (plan §9). Static network first; operational tables follow.
--
-- The matcher reads the network tables into memory at boot; nothing here is on
-- the per-ping hot path.

CREATE EXTENSION IF NOT EXISTS postgis;
CREATE EXTENSION IF NOT EXISTS pg_trgm;   -- stop-name search

-- One row tracking the currently active dataset version (plan §5.3).
CREATE TABLE IF NOT EXISTS dataset_version (
    id            smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    version       text NOT NULL,
    shape_count   integer NOT NULL DEFAULT 0,
    stop_count    integer NOT NULL DEFAULT 0,
    activated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS stops (
    stop_id text PRIMARY KEY,
    name    text NOT NULL,
    geom    geography(Point, 4326) NOT NULL,
    mode    text,
    operator text
);

CREATE TABLE IF NOT EXISTS routes (
    route_id      text PRIMARY KEY,
    short_name    text NOT NULL,
    long_name     text,
    mode          text,
    operator      text,
    route_type    integer,
    color         text
);

CREATE TABLE IF NOT EXISTS shapes (
    shape_id   text PRIMARY KEY,
    route_id   text NOT NULL REFERENCES routes(route_id),
    direction  smallint NOT NULL DEFAULT 0,
    geom       geography(LineString, 4326) NOT NULL,
    length_m   real NOT NULL
);

CREATE TABLE IF NOT EXISTS trips (
    trip_id     text PRIMARY KEY,
    route_id    text NOT NULL REFERENCES routes(route_id),
    shape_id    text NOT NULL REFERENCES shapes(shape_id),
    service_id  text NOT NULL,
    direction   smallint NOT NULL DEFAULT 0
);

-- shape_dist_m is the distance-along-shape axis everything runs on (§5.3).
CREATE TABLE IF NOT EXISTS stop_times (
    trip_id        text NOT NULL REFERENCES trips(trip_id),
    seq            integer NOT NULL,
    stop_id        text NOT NULL REFERENCES stops(stop_id),
    arrival_s      integer NOT NULL,   -- seconds after midnight (>24h allowed)
    departure_s    integer NOT NULL,
    shape_dist_m   real NOT NULL,
    PRIMARY KEY (trip_id, seq)
);

CREATE INDEX IF NOT EXISTS shapes_geom_gix ON shapes USING gist (geom);
CREATE INDEX IF NOT EXISTS stops_geom_gix  ON stops  USING gist (geom);
CREATE INDEX IF NOT EXISTS stops_name_trgm ON stops  USING gin (name gin_trgm_ops);
CREATE INDEX IF NOT EXISTS stop_times_trip_seq ON stop_times (trip_id, seq);

-- --- operational -------------------------------------------------------------
-- Raw pings are short-lived; only aggregates persist (plan §11.1).

CREATE TABLE IF NOT EXISTS sessions (
    token_hash   bytea PRIMARY KEY,          -- sha256 of the rotating token
    started_at   timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    route_id     text,
    trip_id      text,
    ended_at     timestamptz
);

CREATE TABLE IF NOT EXISTS segment_stats (
    shape_id  text NOT NULL,
    seg_idx   integer NOT NULL,
    dow       smallint NOT NULL,
    hour      smallint NOT NULL,
    n         integer NOT NULL DEFAULT 0,
    p50_s     real,
    p90_s     real,
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (shape_id, seg_idx, dow, hour)
);

CREATE TABLE IF NOT EXISTS trip_delays (
    trip_id       text NOT NULL,
    service_date  date NOT NULL,
    stop_id       text NOT NULL,
    observed_arr  timestamptz NOT NULL,
    delay_s       integer NOT NULL,
    n_riders      smallint NOT NULL DEFAULT 1,
    PRIMARY KEY (trip_id, service_date, stop_id)
);

-- User confirmations are ground truth for the matcher.
CREATE TABLE IF NOT EXISTS labels (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    token_hash  bytea NOT NULL,
    ts          timestamptz NOT NULL DEFAULT now(),
    route_id    text NOT NULL,
    direction   smallint NOT NULL,
    source      text NOT NULL
);
