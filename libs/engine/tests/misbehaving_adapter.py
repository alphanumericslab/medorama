#!/usr/bin/env python3
"""Adapters that misbehave, for the tests of the engine's limits (docs/adapters.md, "Limits").
The mode is the program's name: `noisy-adapter` writes much on standard error, `hang-adapter`
never answers `index` or `identify`, `hangdescribe-adapter` never answers `describe`,
`liar-adapter` answers `serve` requests with more samples than asked, `env-adapter` reports
the environment it gets."""
import json, os, struct, sys, time

mode = os.path.basename(sys.argv[0]).split('-')[0]
cmd = sys.argv[1]
if cmd == 'describe':
    if mode == 'hangdescribe':
        time.sleep(60)
    print(json.dumps({'name': mode, 'version': '1', 'protocol': 1,
                      'formats': [{'kind': mode, 'label': mode, 'extensions': [mode]}]}))
elif cmd == 'index':
    if mode == 'hang':
        time.sleep(60)
    sys.stderr.write('w' * 300000 + '\n')
    sys.stderr.flush()
    print(json.dumps({'progress': 0.5}), flush=True)
    with open(sys.argv[2]) as f:
        if 'fail' in f.read():
            sys.stderr.write('the last words\n')
            sys.exit(1)
    with open(os.path.join(sys.argv[3], 'meta.json'), 'w') as f:
        f.write('{}')
    print(json.dumps({'done': True}))
elif cmd == 'identify':
    if mode == 'hang':
        time.sleep(60)
    for p in sys.argv[2:]:
        print(json.dumps({'path': p, 'identifiers': {'full_name': os.environ.get('MEDORAMA_KEY', ''),
                                                     'patient_id': 'PATH' if 'PATH' in os.environ else ''},
                          'others': []}))
elif cmd == 'serve':
    for line in sys.stdin:
        req = json.loads(line)
        k = req['b'] - req['a']
        out = sys.stdout.buffer
        if req['piece'] == 1:
            k += 5  # more than asked
        elif req['piece'] == 2:
            out.write(b'{"n": 1000000000000000000}\n')
            out.flush()
            continue
        elif req['piece'] == 3:
            out.write(b'{"error": "no such piece"}\n')
            out.flush()
            continue
        elif req['piece'] == 5:
            time.sleep(60)  # never answers
        elif req['piece'] == 4:
            out.write(b'not json\n')
            out.flush()
            continue
        out.write(json.dumps({'n': k}).encode() + b'\n')
        out.write(struct.pack('<%dd' % k, *[float(req['a'] + i) for i in range(k)]))
        out.flush()
