-- Cold-start bootstrap for the normalized OTA provider catalog (DB-centric provider
-- architecture, spec 2026-06-29). Replayable, DB-native, insert-if-absent. NOT a place
-- for per-provider facts/status — those are edited via `travel set-ota-*` and live in
-- ota_source_coverage. This file holds only the canonical enums (reference data).

-- Canonical product types (the ONE list coverage + parser_rules reference).
INSERT OR IGNORE INTO product_types (code, description) VALUES
  ('flight',     'Air ticket only'),
  ('hotel',      'Accommodation only'),
  ('fit',        '機+酒 self-guided flight+hotel package (FIT)'),
  ('group_tour', '跟團 escorted group-tour package');

-- Coverage block reasons (why a (source, product_type) is not usable).
INSERT OR IGNORE INTO coverage_block_reasons (code, description) VALUES
  ('renderer_wedge', 'SPA crashes/hangs the headless Chrome renderer (e.g. under WSLg)'),
  ('login_wall',     'Results require an authenticated session'),
  ('captcha',        'Bot-detection captcha blocks automated access'),
  ('cloudflare',     'Cloudflare bot protection blocks access'),
  ('redundant',      'Coverage already provided by another proven source'),
  ('unsupported',    'Source not supported / out of scope');
