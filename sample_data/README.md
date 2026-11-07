# sample_data (Medorama)

Local, **temporary** storage for Medorama's data: synthetic or real databases and the
files they point to, and what the tools write. Everything in this folder except this README is
ignored by git: it is never committed, pushed or pulled, so its contents stay on the machine
where they were put. VitalStream keeps its own, in
[`viewers/vitalstream/sample_data/`](../viewers/vitalstream/sample_data).

```
sample_data/
├── README.md        (the only tracked file)
├── demo/            tools/make-demo-data.py: synthetic patients, their databases and files
│   ├── databases/   the tables to import (CSV, Excel)
│   ├── icu/, holter/, ecg/, imaging/   recordings, ECGs, DICOM images and echo loops
│   └── README.txt
└── outputs/         results of tests and trials
```

A catalog reads files here when its `data_roots` are empty (the default) or include the folder
([docs/administration.md, Settings](../docs/administration.md#settings)). Real patient data
belong on protected storage, not in a clone; if they are put here for a trial, they stay here.
