#!/usr/bin/env python3
"""
Stage 0 aggregator — scrapes flight candidates across destination x duration
for one immutable research run, then hands results to the TS CLI for import.

Performs NO Turso I/O directly: it loads the run via `stage0-export` and writes
via `stage0-import`. All SQL stays in TypeScript (sql-helpers.ts escaping).

For each (destination, duration) pair it checks the seeded scrape-attempt
status — 'ok' pairs are skipped (idempotent re-run), 'pending'/'failed' pairs
are scraped via scrape_date_range.py into a temp file. Results are handed to
`npm run travel -- stage0-import`, which performs all DB writes + ranking.

Temp files are transient implementation detail — not durable state.

Usage:
  python scripts/stage0_research.py --run <run_id>
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile

THIS_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.dirname(THIS_DIR)


def load_run(run_id):
    """Load the run + destinations + durations + scrape attempts via the
    stage0-export CLI command (all SQL stays in TypeScript)."""
    proc = subprocess.run(
        ["npm", "run", "--silent", "travel", "--",
         "stage0-export", "--run", run_id, "--json"],
        check=True, cwd=PROJECT_ROOT, capture_output=True, text=True)
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        print(f"Error: stage0-export did not return JSON for {run_id}",
              file=sys.stderr)
        print(proc.stdout, file=sys.stderr)
        sys.exit(1)


def scrape_pair(run, dest, duration_days):
    """Scrape one (destination, duration) pair via scrape_date_range.py.
    Returns the parsed results list, or raises on failure."""
    with tempfile.NamedTemporaryFile(
            mode="r", suffix=".json", delete=False) as tf:
        tmp_path = tf.name
    try:
        cmd = [
            sys.executable,
            os.path.join(THIS_DIR, "scrape_date_range.py"),
            "--depart-start", run["window_start"],
            "--depart-end", run["window_end"],
            "--origin", run["origin_code"].lower(),
            "--dest", dest["dest_code"].lower(),
            "--duration", str(duration_days),
            "--pax", str(run["pax"]),
            "--exchange-rate", str(run["exchange_rate_usd_twd"]),
            "--output", tmp_path,
        ]
        subprocess.run(cmd, check=True, cwd=PROJECT_ROOT)
        with open(tmp_path, "r", encoding="utf-8") as f:
            return json.load(f).get("results", [])
    finally:
        if os.path.exists(tmp_path):
            os.unlink(tmp_path)


def build_candidates(run, dest, nights, results):
    """Map scrape results -> candidate dicts for stage0-import."""
    candidates = []
    for r in results:
        depart = r.get("depart_date")
        return_date = r.get("return_date")
        if not depart or not return_date:
            continue
        total = r.get("combined_cheapest_twd")
        cand_id = f"{run['run_id']}-{dest['dest_code']}-{depart}-{nights}n"
        flights = []
        for direction, key in (("outbound", "outbound"), ("return", "inbound")):
            leg = r.get(key) or {}
            leg_flights = leg.get("flights") or []
            if not leg_flights:
                continue
            cheapest = min(leg_flights, key=lambda x: x.get("total_usd", 1e9))
            flights.append({
                "direction": direction,
                "airline": cheapest.get("airline"),
                "departTime": cheapest.get("depart"),
                "arriveTime": cheapest.get("arrive"),
                "duration": cheapest.get("duration"),
                "nonstop": cheapest.get("nonstop"),
                "priceTotalTwd": None,
            })
        candidates.append({
            "candidateId": cand_id,
            "runId": run["run_id"],
            "destCode": dest["dest_code"],
            "departDate": depart,
            "returnDate": return_date,
            "nights": nights,
            "flightTotalTwd": int(total) if total is not None else None,
            "leaveDays": None,  # computed by stage0-import (TS leave calculator)
            "verdict": None,
            "flights": flights,
        })
    return candidates


def main():
    parser = argparse.ArgumentParser(description="Stage 0 flight aggregator")
    parser.add_argument("--run", required=True, help="Stage 0 run_id")
    args = parser.parse_args()

    run = load_run(args.run)
    print(f"Stage 0 aggregator — run {run['run_id']} "
          f"({len(run['destinations'])} dest x {len(run['durations'])} duration)")

    # Build a {(dest_code, nights): status} map from the seeded attempt rows.
    attempt_status = {
        (a["dest_code"], int(a["nights"])): a["status"]
        for a in run.get("attempts", [])
    }

    all_candidates = []
    attempts = []
    for dest in run["destinations"]:
        for dur in run["durations"]:
            nights = int(dur["nights"])
            duration_days = int(dur["duration_days"])
            label = f"{dest['dest_code']} {nights}n"
            # Idempotent re-run: skip pairs already scraped successfully.
            if attempt_status.get((dest["dest_code"], nights)) == "ok":
                print(f"  skipping {label} (already ok)")
                continue
            try:
                print(f"  scraping {label} ...")
                results = scrape_pair(run, dest, duration_days)
                cands = build_candidates(run, dest, nights, results)
                all_candidates.extend(cands)
                attempts.append({
                    "runId": run["run_id"], "destCode": dest["dest_code"],
                    "nights": nights, "status": "ok",
                    "candidateCount": len(cands), "error": None,
                })
                print(f"    -> {len(cands)} candidates")
            except Exception as exc:  # noqa: BLE001 — continue other pairs
                print(f"    !! {label} failed: {exc}", file=sys.stderr)
                attempts.append({
                    "runId": run["run_id"], "destCode": dest["dest_code"],
                    "nights": nights, "status": "failed",
                    "candidateCount": None, "error": str(exc)[:500],
                })

    if not all_candidates and not attempts:
        print("All pairs already scraped — nothing to do.")
        print(f"View: npm run travel -- stage0-compare --run {run['run_id']}")
        return

    # Hand off to the TS CLI for all DB writes + leave-days + ranking.
    with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", delete=False, encoding="utf-8") as tf:
        handoff_path = tf.name
        json.dump({"candidates": all_candidates, "attempts": attempts}, tf,
                  ensure_ascii=False)
    try:
        subprocess.run(
            ["npm", "run", "travel", "--", "stage0-import",
             "--run", run["run_id"], "--file", handoff_path],
            check=True, cwd=PROJECT_ROOT)
    finally:
        if os.path.exists(handoff_path):
            os.unlink(handoff_path)

    print(f"Done. View: npm run travel -- stage0-compare --run {run['run_id']}")


if __name__ == "__main__":
    main()
