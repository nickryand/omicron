ALTER TABLE oximeter.fields_ipaddr_local ON CLUSTER oximeter_cluster MODIFY TTL last_updated_at + INTERVAL 30 DAY;
