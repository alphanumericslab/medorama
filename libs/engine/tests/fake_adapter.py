#!/usr/bin/env python3
"""A small adapter for the tests (docs/adapters.md): a made-up `.fake` format, a JSON file of
{fs, t0, label, unit, samples, patient, patient_id}. It answers every command of the protocol."""
import json, os, struct, sys

def load(path):
    with open(path) as f:
        return json.load(f)

cmd = sys.argv[1]
if cmd == 'describe':
    print(json.dumps({'name': 'fake', 'version': '1.0', 'protocol': 1,
                      'formats': [{'kind': 'fake', 'label': 'Fake recording', 'extensions': ['fake']}]}))
elif cmd == 'detect':
    for p in sys.argv[2:]:
        print(json.dumps({'path': p, 'kind': 'fake' if p.endswith('.fake') else None}))
elif cmd == 'index':
    rec, out = load(sys.argv[2]), sys.argv[3]
    meta = {'channels': [{'id': 0, 'label': rec['label'], 'unit': rec['unit'], 'modality': 'ecg'}],
            'pieces': [{'id': 0, 'ch': 0, 'fs': rec['fs'], 'n': len(rec['samples']), 't0': rec['t0'], 'env': []}],
            'vitals': [], 'alarms': [], 'gaps': [], 'untimed': [], 'marker_names': [], 'notes': []}
    for name in ('mk_piece.i32', 'mk_sample.i64', 'mk_code.i16', 'vit_t.f64', 'vit_v.f64'):
        open(os.path.join(out, name), 'wb').close()
    with open(os.path.join(out, 'meta.json'), 'w') as f:
        json.dump(meta, f)
    print(json.dumps({'progress': 0.5}))
    print(json.dumps({'done': True}))
elif cmd == 'identify':
    for p in sys.argv[2:]:
        rec = load(p)
        print(json.dumps({'path': p, 'identifiers': {'full_name': rec['patient'], 'patient_id': rec.get('patient_id', '')},
                          'others': []}))
elif cmd == 'serve':
    for line in sys.stdin:
        req = json.loads(line)
        meta = load(os.path.join(req['index'], 'meta.json'))
        rec = load(meta['path'])
        y = rec['samples'][req['a']:req['b']]
        sys.stdout.write(json.dumps({'n': len(y)}) + '\n')
        sys.stdout.flush()
        sys.stdout.buffer.write(struct.pack('<%dd' % len(y), *y))
        sys.stdout.buffer.flush()
