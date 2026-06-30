-- OTA source URL-token seed: destination-scoped placeholder tokens (spec 2026-07-01).
-- INSERT OR IGNORE: fills an empty catalog, never clobbers a live edit. One statement per line.
-- No semicolons or apostrophes in comment lines or value literals (run_seed_file_stmts splitter).

INSERT OR IGNORE INTO ota_source_url_token (source_id, product_type, placeholder, input_key, input_value, token_value) VALUES ('besttour','group_tour','region_id','destination','tokyo','295');
INSERT OR IGNORE INTO ota_source_url_token (source_id, product_type, placeholder, input_key, input_value, token_value) VALUES ('settour','fit','region_id','destination','tokyo','179900');
INSERT OR IGNORE INTO ota_source_url_token (source_id, product_type, placeholder, input_key, input_value, token_value) VALUES ('settour','fit','dest_code','destination','tokyo','NRT');
INSERT OR IGNORE INTO ota_source_url_token (source_id, product_type, placeholder, input_key, input_value, token_value) VALUES ('eztravel','fit','dest_code','destination','tokyo','TYO');