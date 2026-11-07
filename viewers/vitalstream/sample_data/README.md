# sample_data (VitalStream)

Local, **temporary** storage for VitalStream's data (Medorama has its own, in the repository
root's [`sample_data/`](../../../sample_data)). Recordings used while working on the viewers may
be kept here, and only here. Everything in this folder except this README is ignored by git: it
is never committed, pushed or pulled, so its contents stay on the machine where they were put.

```
sample_data/
├── README.md          (the only tracked file)
├── <recordings>       put recordings here (any sub-folders)
└── outputs/
    └── index/         the viewers' shared index folder, when they run from this clone
                       (safe to delete: rebuilt on demand)
```

Who uses it:
- the three viewers, run from the clone, keep their indexes in `outputs/index/` and show this
  folder as a shortcut in their file browser;
- `analysis_tools/matlab/` reads WFDB collections from `wfdb/`.

The viewers' tests use the synthetic recordings of [`../testdata`](../testdata) instead, never
this folder. On a server where the data already live elsewhere, nothing needs to be copied
here: point the viewers at the data folder.
