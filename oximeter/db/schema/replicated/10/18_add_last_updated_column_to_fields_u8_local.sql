ALTER TABLE oximeter.fields_u8_local ON CLUSTER oximeter_cluster ADD COLUMN IF NOT EXISTS last_updated_at DateTime MATERIALIZED now();
