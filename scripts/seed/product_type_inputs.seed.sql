-- product_type_inputs seed: canonical input contracts per product_type (spec 2026-07-01).
-- INSERT OR IGNORE: fills an empty catalog, never clobbers a live edit. One statement per line.
-- No semicolons or apostrophes in comment lines or value literals (run_seed_file_stmts splitter).

INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('flight','destination','token_key',1,NULL,0);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('flight','depart','common',1,'caller',1);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('flight','return','common',1,'caller',2);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('flight','origin','common',1,'db',3);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('flight','currency','common',1,'db',4);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('hotel','destination','token_key',1,NULL,0);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('hotel','hotel','token_key',1,NULL,1);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('hotel','depart','common',1,'caller',2);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('hotel','nights','common',1,'caller',3);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('hotel','pax','common',1,'caller',4);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('hotel','rooms','common',1,'code',5);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('hotel','currency','common',1,'db',6);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('fit','destination','token_key',1,NULL,0);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('fit','depart','common',1,'caller',1);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('fit','return','common',1,'caller',2);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('fit','pax','common',1,'caller',3);
INSERT OR IGNORE INTO product_type_inputs (product_type, input_name, input_class, required, default_source, sort_order) VALUES ('group_tour','destination','token_key',1,NULL,0);