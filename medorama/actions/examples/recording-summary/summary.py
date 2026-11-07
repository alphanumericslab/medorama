"""Example Medorama action: summarizes a recording from its index.

The catalog runs this in the job's output folder and passes the job description (JSON) on
standard input (also in job.json).  The script:
  - reads only the recording's index (never the recording's folder, never writes there);
  - writes its results into the current folder (channels.csv, summary.json);
  - prints `progress <0..1>` lines;
  - exits with 0 on success (anything else marks the job failed; stderr goes to log.txt).

Standard library only.
"""
import csv
import json
import os
import sys


def main():
    job = json.load(sys.stdin)
    index_dir = job["recording"]["index_dir"]
    if not index_dir:
        print("the recording has no index", file=sys.stderr)
        return 2
    with open(os.path.join(index_dir, "meta.json"), encoding="utf-8") as f:
        meta = json.load(f)
    print("progress 0.3", flush=True)

    pieces = meta.get("pieces", [])
    timed = [p for p in pieces if p.get("timed", True) and p.get("fs")]
    t0 = min((p["t0"] for p in timed), default=None)
    t1 = max((p["t0"] + p["n"] / p["fs"] for p in timed), default=None)
    channels = meta.get("channels", [])
    with open("channels.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["id", "label", "unit", "modality", "pieces", "samples"])
        for c in channels:
            mine = [p for p in pieces if p.get("ch") == c.get("id")]
            w.writerow([c.get("id"), c.get("label"), c.get("unit"), c.get("modality"), len(mine), sum(p.get("n", 0) for p in mine)])
    print("progress 0.8", flush=True)

    limit = int(job["params"].get("max_channels", 64))
    summary = {
        "duration_s": (t1 - t0) if t0 is not None else None,
        "channels": len(channels),
        "channel_labels": [c.get("label") for c in channels][:limit],
        "vital_signs": [v.get("name") for v in meta.get("vitals", [])],
        "gaps": len(meta.get("gaps", [])),
        "encounters": [e["encounter_id"] for e in job["encounters"]],
    }
    with open("summary.json", "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=1)
    print("progress 1", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
