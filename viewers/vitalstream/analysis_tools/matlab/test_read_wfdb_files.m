clear
close all
clc

% Reads one record of a WFDB collection and detects beats on one channel. Needs wfdb-x for
% MATLAB (wfdbx.read_record: addpath of the matlab folder of https://github.com/alphanumericslab/wfdb-x)
% and OSET (peak_det_likelihood_long_recs) on the MATLAB path. Run from this folder.
%
% A WFDB collection (docs/wfdb-collection.md at the repository root) holds its records in
% <collection>/<modality>/<record>.hea/.dat.

wfdb_dir = fullfile('..', '..', 'sample_data', 'wfdb');
collection = 'recording';   % the collection's folder name (set to yours)
fname = fullfile(wfdb_dir, collection, 'ecg', [collection '_ecg_240_0001']);
% fname = fullfile(wfdb_dir, collection, 'resp', [collection '_resp_60_0001']);
% fname = fullfile(wfdb_dir, collection, 'spo2', [collection '_spo2_60_0001']);
% fname = fullfile(wfdb_dir, collection, 'pressure', [collection '_pressure_125_0001']);
ref_ch = 1;

rec = wfdbx.read_record(fname, 'physical', true);   % physical units; NaN where samples are missing
data = rec.signals;
fs = rec.fs;

% wlen = 5;
% threshold = 20;
% fc = 0.2;
% [data_den, data_den_smoothed] = conditional_median_filter(data, wlen, threshold, fc);

% [~, peak_indexes] = peak_det_likelihood_long_recs(data', fs);
[~, peak_indexes] = peak_det_likelihood_long_recs(data(:, ref_ch)', fs);

rr = diff(peak_indexes)/fs;
hr = 60.0./rr;

time = (0:length(data)-1)/fs;

figure
hold on
plot(time, data);
% plot(time, data_den);
grid
xlabel('time(s)')
ylabel('Amplitude(mV)')
set(gca, 'fontsize', 16)

figure
plot(time(peak_indexes(2:end)), hr);
grid
xlabel('time(s)')
ylabel('HR(bpm)')
set(gca, 'fontsize', 16)

figure
hold on
plot(time, data(:, ref_ch))
plot(time(peak_indexes), data(peak_indexes, ref_ch), 'ro', 'markersize', 16)
grid
xlabel('time(s)')
ylabel('Amplitude(mV)')
set(gca, 'fontsize', 16)
