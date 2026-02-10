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
  --morning: #fef9c3;
  --afternoon: #fce7f3;
  --evening: #e0e7ff;
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
}

.day-header {
  padding: 14px 16px 10px;
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
  font-size: 14px;
  font-weight: 600;
  padding: 0 16px 6px;
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

.pill-transit { background: #eff6ff; color: #1d4ed8; }
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

/* Print */
@media print {
  body { max-width: none; padding: 0; }
  .header { position: static; }
  .lang-btn { display: none; }
  .day-card { break-inside: avoid; }
  .badge-urgent { animation: none; }
}
`;
