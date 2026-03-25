CREATE TABLE IF NOT EXISTS candles (
    ts            TIMESTAMPTZ      NOT NULL,
    instrument_id TEXT             NOT NULL,
    interval      TEXT             NOT NULL,
    open          DOUBLE PRECISION NOT NULL,
    high          DOUBLE PRECISION NOT NULL,
    low           DOUBLE PRECISION NOT NULL,
    close         DOUBLE PRECISION NOT NULL,
    volume        DOUBLE PRECISION NOT NULL,
    trade_count   INTEGER          NOT NULL DEFAULT 0
);

SELECT create_hypertable('candles', 'ts', if_not_exists => TRUE);

CREATE UNIQUE INDEX IF NOT EXISTS candles_unique
    ON candles (ts, instrument_id, interval);

CREATE INDEX IF NOT EXISTS candles_instrument_ts
    ON candles (instrument_id, interval, ts DESC);
