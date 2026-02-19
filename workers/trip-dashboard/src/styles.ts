export const CSS = `
:root {
  --bg: #f8f7f4;
  --card: #ffffff;
  --text: #1a1a1a;
  --text-dim: #6b6b6b;
  --border: #e8e6e1;
  --accent: #2563eb;
  --accent-light: #dbeafe;
  --green: #16a34a;
  --green-bg: #dcfce7;
  --amber: #d97706;
  --amber-bg: #fef3c7;
  --red: #dc2626;
  --red-bg: #fee2e2;
  --morning: #fde68a;
  --afternoon: #fbcfe8;
  --evening: #c7d2fe;
  --radius: 12px;
  --shadow: 0 1px 3px rgba(0,0,0,0.08), 0 1px 2px rgba(0,0,0,0.04);
  --shadow-lg: 0 4px 12px rgba(0,0,0,0.1);
}

* { margin: 0; padding: 0; box-sizing: border-box; }

body {
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Noto Sans TC', 'Noto Sans JP', sans-serif;
  background: var(--bg);
  color: var(--text);
  line-height: 1.5;
  -webkit-font-smoothing: antialiased;
  max-width: 640px;
  margin: 0 auto;
  padding: 0 12px 48px;
}

/* Header */
.header {
  position: sticky;
  top: 0;
  z-index: 10;
  background: var(--card);
  border-bottom: 1px solid var(--border);
  padding: 12px 16px;
  margin: 0 -12px;
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.header h1 {
  font-size: 18px;
  font-weight: 700;
  letter-spacing: -0.3px;
}

.header-sub {
  font-size: 12px;
  color: var(--text-dim);
  margin-top: 2px;
}

.lang-btn {
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 20px;
  padding: 4px 12px;
  font-size: 13px;
  cursor: pointer;
  text-decoration: none;
  color: var(--text);
  white-space: nowrap;
  flex-shrink: 0;
  margin-left: 12px;
}

/* Plan nav */
.plan-nav {
  display: flex;
  gap: 6px;
  padding: 8px 16px;
  margin: 0 -12px;
  background: var(--bg);
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
}

.plan-pill {
  padding: 6px 16px;
  border-radius: 20px;
  font-size: 13px;
  font-weight: 500;
  text-decoration: none;
  color: var(--text-dim);
  background: var(--card);
  border: 1px solid var(--border);
  white-space: nowrap;
  min-height: 32px;
  display: inline-flex;
  align-items: center;
}

.plan-pill:active {
  background: var(--accent-light);
  transform: scale(0.97);
}

.plan-pill-active {
  background: var(--accent);
  color: white;
  border-color: var(--accent);
}

.route-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 20px;
  padding: 4px 12px;
  font-size: 12px;
  font-weight: 500;
  text-decoration: none;
  color: var(--accent);
  white-space: nowrap;
  min-height: 28px;
}

.route-btn:active {
  background: var(--accent-light);
  transform: scale(0.97);
}

/* Booking summary */
.booking-summary {
  background: var(--card);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  padding: 16px;
  margin-top: 16px;
}

.booking-summary h2 {
  font-size: 14px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-dim);
  margin-bottom: 12px;
}

.booking-grid {
  display: grid;
  gap: 10px;
}

.booking-item {
  display: flex;
  gap: 10px;
  align-items: flex-start;
}

.booking-icon {
  flex-shrink: 0;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  background: var(--accent-light);
}

.booking-detail {
  flex: 1;
  min-width: 0;
}

.booking-label {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.4px;
  color: var(--text-dim);
}

.booking-value {
  font-size: 14px;
  font-weight: 500;
}

.booking-sub {
  font-size: 12px;
  color: var(--text-dim);
}

/* Status badges */
.badge {
  display: inline-block;
  font-size: 11px;
  font-weight: 600;
  padding: 2px 8px;
  border-radius: 10px;
  white-space: nowrap;
}

.badge-booked { background: var(--green-bg); color: var(--green); }
.badge-planned { background: var(--accent-light); color: var(--accent); }
.badge-pending { background: var(--amber-bg); color: var(--amber); }
.badge-urgent { background: var(--red-bg); color: var(--red); animation: pulse 2s ease-in-out infinite; }

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.7; }
}

/* Alert card */
.alert {
  background: var(--amber-bg);
  border: 1px solid var(--amber);
  border-radius: var(--radius);
  padding: 12px 16px;
  margin-top: 16px;
  display: flex;
  gap: 10px;
  align-items: flex-start;
}

.alert-urgent {
  background: var(--red-bg);
  border-color: var(--red);
}

.alert-icon { font-size: 18px; flex-shrink: 0; }

.alert-text {
  font-size: 13px;
  font-weight: 500;
}

.alert-text a {
  color: var(--accent);
  text-decoration: underline;
}

/* Day cards */
.day-card {
  background: var(--card);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  margin-top: 16px;
  overflow: hidden;
  border-left: 4px solid var(--accent);
}

.day-card-arrival { border-left-color: var(--accent); }
.day-card-departure { border-left-color: var(--amber); }
.day-card-full { border-left-color: var(--green); }

.day-header {
  padding: 16px 16px 10px;
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
}

.day-number {
  font-size: 28px;
  font-weight: 800;
  line-height: 1;
  color: var(--accent);
}

.day-date {
  font-size: 13px;
  color: var(--text-dim);
  margin-top: 2px;
}

.day-theme {
  font-size: 16px;
  font-weight: 600;
  padding: 0 16px 8px;
}

.day-type-badge {
  font-size: 11px;
  font-weight: 600;
  background: var(--accent-light);
  color: var(--accent);
  padding: 2px 8px;
  border-radius: 10px;
}

/* Weather strip */
.weather-strip {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 16px;
  background: linear-gradient(135deg, #f0f4ff 0%, #e8f0fe 100%);
  font-size: 13px;
  border-top: 1px solid var(--border);
  border-bottom: 1px solid var(--border);
}

.weather-icon { font-size: 20px; }

.weather-temp {
  font-weight: 600;
}

.weather-rain {
  margin-left: auto;
  color: var(--accent);
  font-weight: 500;
}

.weather-feels {
  font-size: 11px;
  color: var(--text-dim);
  white-space: nowrap;
}

.weather-clothing {
  padding: 6px 16px 8px;
  font-size: 12px;
  color: #4a5568;
  background: linear-gradient(135deg, #fefce8 0%, #fef3c7 100%);
  border-bottom: 1px solid var(--border);
  line-height: 1.4;
}

/* Session blocks */
.session {
  padding: 12px 16px;
  border-top: 1px solid var(--border);
}

.session:first-of-type { border-top: none; }

.session-label {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 8px;
  padding: 2px 8px;
  border-radius: 4px;
}

.session-morning .session-label { background: var(--morning); color: #92400e; }
.session-afternoon .session-label { background: var(--afternoon); color: #9d174d; }
.session-evening .session-label { background: var(--evening); color: #3730a3; }

.session-focus {
  font-size: 15px;
  font-weight: 600;
  margin-bottom: 6px;
}

.activity-list {
  list-style: none;
  padding: 0;
}

.activity-list li {
  font-size: 13px;
  padding: 3px 0;
  padding-left: 18px;
  position: relative;
  color: var(--text);
}

.activity-list li::before {
  content: '';
  position: absolute;
  left: 4px;
  top: 10px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--border);
}

.activity-booking {
  background: var(--amber-bg);
  border-radius: 6px;
  padding: 2px 6px;
  font-weight: 500;
}

/* Transit & meal pills */
.info-pills {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 8px;
}

.pill {
  font-size: 11px;
  padding: 3px 8px;
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.pill-transit { background: #eff6ff; color: #1d4ed8; text-decoration: none; border: 1px solid #bfdbfe; }
.pill-transit:active { background: #dbeafe; transform: scale(0.97); }
.pill-meal { background: #fef3c7; color: #92400e; }

/* Transit summary */
.transit-summary {
  background: var(--card);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  padding: 16px;
  margin-top: 16px;
}

.transit-summary h2 {
  font-size: 14px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-dim);
  margin-bottom: 12px;
}

.transit-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.transit-table th {
  text-align: left;
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.4px;
  color: var(--text-dim);
  padding: 4px 6px;
  border-bottom: 1px solid var(--border);
}

.transit-table td {
  padding: 6px;
  border-bottom: 1px solid var(--border);
}

.transit-table tr:last-child td { border-bottom: none; }

/* Packing notes */
.packing-card {
  background: var(--card);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  padding: 16px;
  margin-top: 16px;
}

.packing-card h2 {
  font-size: 14px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-dim);
  margin-bottom: 10px;
}

.packing-card li {
  font-size: 13px;
  padding: 3px 0;
  color: var(--text);
}

/* Footer */
.footer {
  margin-top: 24px;
  text-align: center;
  font-size: 11px;
  color: var(--text-dim);
  padding: 12px;
}

/* Map embed */
.map-details { border-top: 1px solid var(--border); }
.map-summary {
  padding: 10px 16px;
  font-size: 13px;
  font-weight: 500;
  color: var(--accent);
  cursor: pointer;
  list-style: none;
  display: flex;
  align-items: center;
  gap: 6px;
}
.map-summary::-webkit-details-marker { display: none; }
.map-summary::after { content: '\u25B8'; margin-left: auto; transition: transform 0.2s; font-size: 12px; color: var(--text-dim); }
details[open] .map-summary::after { transform: rotate(90deg); }
.map-container { width: 100%; overflow: hidden; }
.map-container iframe { display: block; width: 100%; min-height: 250px; max-height: 350px; }
.map-segment-label {
  padding: 8px 16px 4px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text);
  display: flex;
  align-items: center;
  gap: 4px;
}
.map-links {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 4px 0;
}
.map-place-link {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 10px 16px;
  font-size: 13px;
  font-weight: 500;
  color: var(--accent);
  text-decoration: none;
  border-bottom: 1px solid var(--border);
  min-height: 44px;
}
.map-place-link:last-child { border-bottom: none; }
.map-place-link:active { background: var(--accent-light); }
@media (max-width: 480px) { .map-container iframe { min-height: 200px; } }

/* Route legs section */
.route-legs-section {
  margin: 0 0 4px 0;
  padding: 10px 16px 6px;
  border-bottom: 1px solid var(--border);
}
.route-legs-label {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-dim);
  text-transform: uppercase;
  letter-spacing: 0.5px;
  margin-bottom: 6px;
}
.route-leg {
  display: flex;
  align-items: baseline;
  gap: 6px;
  padding: 3px 0;
  font-size: 13px;
  flex-wrap: wrap;
}
.route-leg-icon { flex-shrink: 0; }
.route-leg-places {
  color: var(--text);
  text-decoration: none;
  font-weight: 500;
  flex: 1;
}
.route-leg-places:hover { text-decoration: underline; color: var(--accent); }
.route-leg-time {
  font-size: 12px;
  font-weight: 600;
  color: var(--accent);
  white-space: nowrap;
}
.route-leg-dur {
  font-size: 12px;
  color: var(--text-dim);
  white-space: nowrap;
}
.route-leg-notes {
  font-size: 12px;
  color: var(--text-dim);
  font-style: italic;
}

/* Plan index */
.plan-index {
  display: grid;
  gap: 12px;
  margin-top: 16px;
}

.plan-card {
  display: block;
  background: var(--card);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
  padding: 20px;
  text-decoration: none;
  color: var(--text);
  transition: box-shadow 0.15s, transform 0.15s;
}

.plan-card:active {
  box-shadow: var(--shadow-lg);
  transform: translateY(-1px);
}

.plan-card-name {
  font-size: 18px;
  font-weight: 700;
  margin-bottom: 6px;
}

.plan-card-dates {
  font-size: 14px;
  color: var(--text-dim);
}

.plan-card-days {
  display: inline-block;
  margin-top: 8px;
  font-size: 12px;
  font-weight: 600;
  background: var(--accent-light);
  color: var(--accent);
  padding: 2px 10px;
  border-radius: 10px;
}

/* Edit mode */
.edit-btn {
  background: none;
  border: none;
  cursor: pointer;
  font-size: 14px;
  padding: 2px 4px;
  opacity: 0.4;
  transition: opacity 0.15s;
  vertical-align: middle;
  line-height: 1;
}
.edit-btn:hover { opacity: 1; }

.edit-wrap { position: relative; }

.edit-field {
  display: none;
  margin-top: 6px;
}

.edit-field.active {
  display: block;
}

.edit-textarea {
  width: 100%;
  min-height: 60px;
  font-family: inherit;
  font-size: 13px;
  padding: 8px;
  border: 2px solid var(--accent);
  border-radius: 8px;
  resize: vertical;
  background: var(--card);
  color: var(--text);
  line-height: 1.5;
}

.edit-textarea:focus {
  outline: none;
  box-shadow: 0 0 0 3px var(--accent-light);
}

.edit-actions {
  display: flex;
  gap: 6px;
  margin-top: 6px;
}

.edit-save, .edit-cancel {
  font-size: 12px;
  font-weight: 600;
  padding: 4px 14px;
  border-radius: 6px;
  border: none;
  cursor: pointer;
  min-height: 32px;
}

.edit-save {
  background: var(--accent);
  color: white;
}

.edit-save:active { opacity: 0.8; }

.edit-cancel {
  background: var(--bg);
  color: var(--text-dim);
  border: 1px solid var(--border);
}

.edit-status {
  font-size: 11px;
  padding: 2px 6px;
  border-radius: 4px;
  margin-left: 8px;
  display: inline-block;
}

.edit-status-ok {
  background: var(--green-bg);
  color: var(--green);
}

.edit-status-err {
  background: var(--red-bg);
  color: var(--red);
}

@keyframes flash-green {
  0% { background: var(--green-bg); }
  100% { background: transparent; }
}

.flash-success {
  animation: flash-green 1.5s ease-out;
}

/* Print */
@media print {
  body { max-width: none; padding: 0; }
  .header { position: static; }
  .lang-btn { display: none; }
  .day-card { break-inside: avoid; }
  .badge-urgent { animation: none; }
  .map-details { display: none; }
  .edit-btn { display: none; }
  .edit-field { display: none !important; }
}
`;
