-- OTA provider coverage and region-code seed (DB-centric provider architecture, spec 2026-06-29).
-- Cold-start reproducible source of the per (source, product_type) coverage matrix and region maps.
-- Replaces the manual set-ota-coverage / set-ota-region CLI runs that were not reproducible from the
-- tree (review finding F1, 2026-06-29). INSERT OR IGNORE: fills an empty catalog, never clobbers a
-- live edit. Each INSERT is one line and comments are whole-line only (no inline punctuation), since
-- the migrate splitter splits on the statement separator before stripping comment lines.

-- coverage: flight
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('google_flights','flight',1,'2026-06-26','agent_parse','https://www.google.com/travel/flights?q=Flights+to+{dest}+from+{origin}+on+{depart_date}+through+{return_date}&curr={currency}&hl=zh-TW',NULL);
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('skyscanner','flight',0,NULL,NULL,NULL,'captcha');
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('tigerair','flight',0,NULL,NULL,NULL,'redundant');
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('trip','flight',0,NULL,NULL,NULL,'redundant');
-- coverage: fit
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('eztravel','fit',1,'2026-06-26','agent_parse','https://packages.eztravel.com.tw/roundtrip-TPE-{dest_code}?checkin={depart_date}&checkout={return_date}&adult={pax}&child=0',NULL);
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('settour','fit',1,'2026-06-26','agent_parse','https://fit.settour.com.tw/product/v2',NULL);
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('liontravel','fit',0,NULL,NULL,NULL,'renderer_wedge');
-- coverage: group_tour
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('besttour','group_tour',1,'2026-06-26','agent_parse','https://www.besttour.com.tw/e_web/search?v=//////{region_id}///////',NULL);
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('travel4u','group_tour',1,'2026-06-26','agent_parse','https://www.travel4u.com.tw/group/area/{area_code}/japan/',NULL);
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('lifetour','group_tour',0,NULL,NULL,NULL,'renderer_wedge');
-- coverage: hotel
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('agoda','hotel',1,'2026-06-26','agent_parse','https://www.agoda.com/{hotel_slug}/hotel/{city_slug}-{country}.html?checkIn={checkin}&los={nights}&adults={adults}&rooms={rooms}&currency={currency}',NULL);
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('booking','hotel',0,NULL,NULL,NULL,'cloudflare');
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('jalan','hotel',0,NULL,NULL,NULL,'unsupported');
INSERT OR IGNORE INTO ota_source_coverage (source_id, product_type, proven, proven_at, method, search_url, blocked_reason_code) VALUES ('rakuten_travel','hotel',0,NULL,NULL,NULL,'unsupported');

-- region codes: besttour
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('besttour','group_tour','東京','295');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('besttour','group_tour','關東','28');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('besttour','group_tour','北海道','26');
-- region codes: travel4u
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('travel4u','group_tour','東京｜東北','41');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('travel4u','group_tour','大阪｜四國','40');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('travel4u','group_tour','北海道','39');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('travel4u','group_tour','九州','42');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('travel4u','group_tour','沖繩','43');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('travel4u','group_tour','北陸｜名古屋','63');
INSERT OR IGNORE INTO ota_source_region_codes (source_id, product_type, region_label, region_code) VALUES ('travel4u','group_tour','mixed','178');
