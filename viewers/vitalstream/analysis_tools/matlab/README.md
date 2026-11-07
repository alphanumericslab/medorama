# MATLAB analysis scripts

This folder holds MATLAB scripts that read the records of a
[WFDB collection](../../../../docs/wfdb-collection.md). It is part of VitalStream's
[analysis tools](../README.md).

| Script | What it does |
|---|---|
| `test_read_wfdb_files.m` | Reads one record of a WFDB collection and detects the beats on one channel, then plots the signal, the heart rate and the detected peaks. |

The scripts need these libraries on the MATLAB path:
- [wfdb-x](https://github.com/alphanumericslab/wfdb-x) for MATLAB (`wfdbx.read_record`): add
  its `matlab` folder to the path with `addpath('/path/to/wfdb-x/matlab')`;
- [OSET](https://github.com/alphanumericslab/OSET) (`peak_det_likelihood_long_recs`).

To run a script:
1. Set `collection` and `fname` at the top of the script.
2. Run it from this folder. It reads records from `sample_data/wfdb/<collection>/<modality>/`.
