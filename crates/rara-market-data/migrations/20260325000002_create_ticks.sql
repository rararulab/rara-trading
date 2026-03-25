CREATE TABLE IF NOT EXISTS ticks (
    ts            TIMESTAMPTZ      NOT NULL,
    instrument_id TEXT             NOT NULL,
    price         DOUBLE PRECISION NOT NULL,
    amount        DOUBLE PRECISION NOT NULL,
    side          SMALLINT         NOT NULL
);

SELECT create_hypertable('ticks', 'ts', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS ticks_instrument_ts
    ON ticks (instrument_id, ts DESC);
