# Actions: work on recordings

This page describes actions: work that Medorama does on recordings in the background. It is
written for administrators who run and install actions, and for developers who write plug-ins.

An **action** does something to one recording. Medorama's background workers run it as a
**job**, and keep its result. One action is built in; **plug-ins** add others without a new
build of Medorama. How to run actions from the page is described in
[user-guide.md, Actions and jobs](user-guide.md#actions-and-jobs).

## Contents


- [Built-in actions](#built-in-actions)
- [Runs and jobs](#runs-and-jobs)
- [Plug-ins](#plug-ins)
- [The contract](#the-contract)
- [Privacy](#privacy)

## Built-in actions

| Action | What it does |
|---|---|
| `index` ("Index") | Builds the index the viewers draw from, into the index store of the recording's database (`<workspace>/db-NNNN/index.sqlite`, encrypted with the database's key; index format 3: [index-format.md](../viewers/vitalstream/docs/index-format.md#version-3-the-index-store)), with the recording engine's WFDB or EDF reader, or with the recording's adapter ([readers.md](readers.md)). It runs automatically for new and changed recordings of the connected recording databases while the server runs (`auto_index`). |

A recording is a row of a connected recording database: a db-builder database's `recordings`
table, or a `recordings` view ([databases.md](databases.md)). Medorama numbers each one
`database × 1,000,000,000 + n`, the same number on the page, in the jobs and in the viewer.

**A Holter's heart rate** is not an action of its own: the `index` job computes it after
indexing a recording that has an ECG lead and no heart rate of its own. A Pan–Tompkins detector
finds the QRS complexes, reading an hour of signal at a time. For each minute, the job then keeps
the median of the beat-to-beat rates and their 5th–95th percentile range; minutes with fewer than
10 plausible beats (30–250 bpm) are left out. The series is kept in the recording database's
folder of the workspace (`recordings.sqlite`, encrypted with that database's key) and shown on
the patient page.

## Runs and jobs

**A run** applies one or more actions to a selection: recordings, patients, a cohort's members
(their recordings), or every recording. Only the recordings of the databases the user is granted
`view` are selected. It is expanded to one job per action and recording:
- Missing recordings, and recordings of kinds an action does not apply to, are left out.
- The actions that an action requires are added.
- **No work is done twice.** A job is not queued when the same action version, with the same
  parameters, already succeeded on the recording as it is now (its fingerprint). *Run again
  where already done* (`force`) overrides this for the chosen actions. A job already queued or
  running is never queued twice; asking again with a higher priority raises its priority.

**Workers** (those of `serve`, and `medorama work`, which stops when the queue is empty) take the
job with the highest priority first, then the oldest. "Index now" has priority 10, automatic
indexing 0. A job waits until the actions it requires have succeeded on the recording, and fails
if one of them failed.

**Robustness.** A running job's worker writes a heartbeat every 2 seconds. A job whose heartbeat
stops for 2 minutes (a crashed or killed worker) goes back to the queue. A heartbeat the control
store refuses (busy, a connection lost) is tried again: only the job's row stops a job (cancelled, or
taken back by the queue). A job's end is written again, for up to half a minute, until the
control store takes it. A server or `medorama work` that stops (Ctrl+C, SIGTERM) stops its plug-ins,
with everything they started (TERM to the plug-in's process group, KILL 5 seconds later), and
puts their jobs back in the queue at once. Cancelling stops a plug-in at once; indexing runs to
its end and its result is discarded. Several processes (the server, `work`, the commands) can
share one control store, and each job is taken by exactly one worker.

The following commands queue runs, work through the queue, and list the failed jobs:

```bash
medorama run index --all                       # queue
medorama run recording-summary --cohort 7 --param recording-summary.max_channels=16
medorama work                                  # run the queue here, until it is empty
medorama jobs --state failed                   # what failed, and why
```

## Plug-ins

A plug-in is a folder with an `action.toml` file and a program, in one of the `actions_dirs`
(default `<home>/actions`). Medorama reads the plug-ins when it starts, and again on
`POST api/v1/actions/reload` (administrators). The Actions tab lists them. An example in Python,
using the standard library only, is
[`medorama/actions/examples/recording-summary`](../medorama/actions/examples/recording-summary):
it reads the recording's WFDB headers (never its signals) and writes `channels.csv` and a
`summary.json` of its records, channels and duration.
An `action.toml` file has this form:

```toml
name = "heart_rate"                 # a-z, 0-9, _ and -
title = "Extract heart rate"
version = "1"                       # change it whenever results would differ
description = "…"
scope = "recording"                 # "encounter" is reserved: such actions are listed but cannot run yet
kinds = ["wfdb"]                    # the recording kinds it applies to: wfdb, edf, an adapter's (default: all)
requires = ["index"]                # must have succeeded on the recording first
command = ["python3", "hr.py"]      # relative paths that exist in the action folder are made absolute
timeout_s = 3600                    # the time limit (default: 21600, 6 hours); 0: no limit
default = false                     # ticked in the Actions tab
[params]                            # default parameters; a run may override them
window_s = 10
```

To try the example, add `"/path/to/the/clone/medorama/actions/examples"` to `actions_dirs` in
`medorama.toml`, restart the server, and run `medorama run recording-summary --all && medorama
work`. On Windows, change its `command` to `["python", "summary.py"]`.

## The contract

For each job, Medorama:
1. creates the output folder `<workspace>/db-NNNN/results/<name>/r<n>/j<job id>/`, in the
   recording database's folder of the workspace (`n` is the recording's number in its database;
   a job run again, after its worker stopped, gets a new one: `j<job id>-2/`, `-3/` …, the
   earlier attempt's files kept). The folder goes with its database when the database is
   removed;
2. runs `command` there, with the job description as JSON on standard input, also saved as
   `job.json` (its path in `$MEDORAMA_JOB`);
3. sends the command's standard error to `log.txt`.

The command must follow these rules:
- It writes its results into its working folder, never anywhere else.
- It may print `progress <0..1>` lines on standard output.
- It may leave a `summary.json` (up to 256 KB). Medorama stores it with the job and returns it
  with the recording through the API (`GET recordings/{id}`); the page does not show it. The
  numbers of the summary (its numbers and booleans, and those of its objects one level down:
  `{"rmssd": 41.2, "qc": {"ok": 1}}` gives `rmssd` and `qc.ok`) are also kept in
  `results-<name>.sqlite` in the recording database's folder of the workspace, encrypted with
  that database's key. They can then be searched as measurements named `<name>.<field>`: in the
  criteria builder by that name, and in an expression as `hrv.rmssd > 50`. A word with a dot
  names a database first: when a database's name begins with the action's, write the result in
  quotes, `"hrv.rmssd" > 50`, or after the database's name, `icu."hrv.rmssd" > 50`.
- It exits with 0 on success. Any other exit, a timeout or a cancellation marks the job failed;
  the failure message ends with the last lines of `log.txt`. At the time limit or a cancellation,
  the plug-in's process group gets TERM, then KILL 5 seconds later: a plug-in that must clean up
  handles TERM.

The job description has this form:

```json
{
  "medorama": "medorama 0.2.1",
  "job_id": 12, "action": "heart_rate", "version": "1",
  "params": {"window_s": 10},
  "recording": {"id": 7000000003, "database": "icu", "path": "/data/…/ICU7_12-…", "kind": "wfdb", "name": "ICU7_12-…",
                "format": "WFDB collection", "t_start": 1704106200.0, "t_end": 1704297600.0},
  "patient": "MO-AB4C-9D2E",
  "output_dir": "/workspace/db-0007/results/heart_rate/r3/j12"
}
```

`recording.database` is the name of the recording's database, and `patient` the patient's
pseudonym. A plug-in reads the recording itself, at `recording.path`: the indexes are in the
database's encrypted index store, whose key plug-ins are not given. An action that must run after
indexing (for the recording's times and format to be known) declares `requires = ["index"]`.

## Privacy

The job description names the patient by pseudonym, but it holds the recording's real path, and
the plug-in reads the recording's files as they are on disk: their names, folders and headers
may identify the patient. A plug-in therefore handles patient data, as the server does. Medorama
shows what it writes back through the privacy gate (in Privacy Mode, and for databases a user
holds without `phi`): a summary's times are moved, under keys of its own too, every linked
patient's names are replaced, and an error is only `failed`; a plug-in should still write no
identifier into its summary. A plug-in runs with the rights of Medorama's account. Install only
plug-ins you trust, keep the actions folders writable by that account only, and review what a
plug-in writes to its output folder. The files a plug-in writes there are not encrypted (only the
numbers of its summary, in `results-<name>.sqlite`, are), and they are outside the privacy gate
([security.md, Background jobs and plug-in actions](security.md#13-background-jobs-and-plug-in-actions)).
