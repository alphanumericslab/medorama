"""Example Medorama action: summarizes a WFDB recording from its headers.

Medorama runs this in the job's output folder and passes the job description (JSON) on
standard input (also in job.json; docs/actions.md).  The script:
  - reads only the recording's WFDB headers (`recording.path`: a `.hea` file, or a folder of
    records), never its signals, and never writes beside it;
  - writes its results into the current folder (channels.csv, summary.json);
  - prints `progress <0..1>` lines;
  - exits with 0 on success (anything else marks the job failed; stderr goes to log.txt).

Recordings' indexes are kept encrypted in Medorama's workspace (index format 3): a plug-in reads
the recording itself.  Standard library only.
"""
import csv
import json
import os
import sys


def headers(path):
    """The WFDB headers of a recording: the file itself, or every `.hea` below a folder."""
    if os.path.isfile(path):
        return [path]
    found = []
    for folder, dirs, files in os.walk(path):
        dirs[:] = sorted(d for d in dirs if not d.startswith("."))
        found += [os.path.join(folder, f) for f in sorted(files) if f.endswith(".hea") and not f.startswith(".")]
    return found


def record(hea):
    """(record name, frequency, samples, signals [(label, unit)]) from a header, or None for a
    multi-segment record's layout (its segments are records of their own)."""
    with open(hea, encoding="latin-1") as f:
        lines = [ln.strip() for ln in f if ln.strip() and not ln.startswith("#")]
    if not lines:
        return None
    head = lines[0].split()
    name, nsig = head[0], int(head[1])
    if "/" in name:
        return None
    fs = float(head[2].split("/")[0]) if len(head) > 2 else 250.0
    n = int(head[3]) if len(head) > 3 else 0
    signals = []
    for ln in lines[1:1 + nsig]:
        parts = ln.split()
        gain = parts[2] if len(parts) > 2 else ""
        unit = gain.split("/", 1)[1] if "/" in gain else ""
        signals.append((" ".join(parts[8:]) if len(parts) > 8 else f"signal {len(signals)}", unit))
    return name, fs, n, signals


def main():
    job = json.load(sys.stdin)
    records = [r for r in (record(h) for h in headers(job["recording"]["path"])) if r]
    if not records:
        print("no WFDB record found", file=sys.stderr)
        return 2
    print("progress 0.5", flush=True)

    with open("channels.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["record", "label", "unit", "fs", "samples"])
        for name, fs, n, signals in records:
            for label, unit in signals:
                w.writerow([name, label, unit, fs, n])
    print("progress 0.8", flush=True)

    limit = int(job["params"].get("max_channels", 64))
    labels = []
    for _, _, _, signals in records:
        labels += [s[0] for s in signals if s[0] not in labels]
    summary = {
        "records": len(records),
        "channels": len(labels),
        "channel_labels": labels[:limit],
        # the longest record (records may run side by side)
        "duration_s": max(n / fs for _, fs, n, _ in records if fs > 0),
        # the patient's pseudonym only: never an identifier
        "patient": job.get("patient", ""),
    }
    with open("summary.json", "w", encoding="utf-8") as f:
        json.dump(summary, f, indent=1)
    print("progress 1", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
