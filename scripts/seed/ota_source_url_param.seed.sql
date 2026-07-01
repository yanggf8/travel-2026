-- OTA source URL-parameter seed: destination-scoped URL param values (spec 2026-07-01).
-- One row per (source, product_type, url_param_name, input_name, input_value): when our internal
-- input (input_name=input_value) is set, the URL param (url_param_name) takes url_value.
-- INSERT OR IGNORE: fills an empty catalog, never clobbers a live edit. One statement per line.
-- No semicolons or apostrophes in comment lines or value literals (run_seed_file_stmts splitter).

INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('besttour','group_tour','region_id','destination','tokyo','295');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('settour','fit','region_id','destination','tokyo','179900');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('settour','fit','dest_code','destination','tokyo','NRT');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('eztravel','fit','dest_code','destination','tokyo','TYO');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('travel4u','group_tour','area_code','destination','tokyo','41');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('google_flights','flight','dest','destination','tokyo','Tokyo');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('agoda','hotel','city_slug','destination','tokyo','tokyo');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('agoda','hotel','country','destination','tokyo','jp');
INSERT OR IGNORE INTO ota_source_url_param (source_id, product_type, url_param_name, input_name, input_value, url_value) VALUES ('agoda','hotel','hotel_slug','hotel','shinjuku-washington-hotel-main-building','shinjuku-washington-hotel-main-building');
