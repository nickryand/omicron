CREATE TABLE IF NOT EXISTS oximeter.measurements_histogrami32_local ON CLUSTER oximeter_cluster
(
    timeseries_name String,
    timeseries_key UInt64,
    start_time DateTime64(9, 'UTC'),
    timestamp DateTime64(9, 'UTC'),
    bins Array(Int32),
    counts Array(UInt64),
    min Int32,
    max Int32,
    sum_of_samples Int64,
    squared_mean Float64,
    p50_marker_heights Array(Float64),
    p50_marker_positions Array(UInt64),
    p50_desired_marker_positions Array(Float64),
    p90_marker_heights Array(Float64),
    p90_marker_positions Array(UInt64),
    p90_desired_marker_positions Array(Float64),
    p99_marker_heights Array(Float64),
    p99_marker_positions Array(UInt64),
    p99_desired_marker_positions Array(Float64)
)
ENGINE = ReplicatedMergeTree('/clickhouse/tables/{shard}/measurements_histogrami32_local', '{replica}')
ORDER BY (timeseries_name, timeseries_key, start_time, timestamp)
TTL toDateTime(timestamp) + INTERVAL 30 DAY;
