#!/usr/bin/env python3
"""Naming checks.

    check-naming.py               # the reviewed legacy-name budget
    check-naming.py --inventory   # refresh that budget for review
    check-naming.py --paths       # every public item of `ponko` has one path
    check-naming.py --paths --json FILE   # the same, from rustdoc JSON already built
    check-naming.py --selftest    # check the path walk with made-up input

`--paths` (STABILITY.md, "The crate root and globs"): an item of the engine
is public at exactly one path, under one name, so rustdoc, compiler errors and
the guide all show the same name. Allowed besides it: the prelude, which
re-exports common items as the same items; `#[deprecated]` aliases (their own
items, kept for one minor); and items of the ECS crate, whose one way in is
`ponko::ecs_core` (plus the prelude). The walk reads rustdoc's JSON of the
crate with the docs.rs features, built with the nightly that
scripts/api-snapshots.py pins (CI runs it in the API snapshots job).
"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
INVENTORY = ROOT / 'naming-inventory.json'
LEGACY = re.compile('crabby', re.IGNORECASE)


def references():
    paths = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=ROOT).decode().split('\0')
    result = {}
    for name in sorted(set(paths)):
        path = ROOT / name
        if not name or name == INVENTORY.name or not path.is_file():
            continue
        try:
            count = len(LEGACY.findall(path.read_text()))
        except UnicodeDecodeError:
            count = 0
        path_count = len(LEGACY.findall(name))
        if count or path_count:
            result[name] = {'occurrences': count, 'path_occurrences': path_count}
    return result


def classify(name):
    if name.endswith('README.md') or name in ('AGENTS.md', 'CLAUDE.md', 'CHANGELOG.md', 'NOTICE', 'LICENSE-MIT') or name.endswith(('CONTENT-NOTICES.txt', '/NOTICE')) or name.startswith('changelog.d/'):
        return 'intentional history', 'Preserved project instructions, README text, release history or legal attribution.'
    if '/fixtures/' in name:
        return 'intentional history', 'Immutable historical ABI or saved-format baseline and its provenance/generator.'
    if name.startswith('changelog.d/'):
        return 'intentional history', 'Release notes that name the removed legacy names.'
    if name.startswith('book/') or name == 'STABILITY.md':
        return 'deprecated compatibility', 'Migration instructions and deprecation schedule for legacy names.'
    if name.startswith('src/') or name == 'crates/ponko_mcp/src/lib.rs':
        return 'deprecated compatibility', 'Legacy input fallback, protocol interoperability alias or deprecated Rust alias and its regression coverage.'
    if name.startswith(('tests/', 'crates/ponko_ffi/tests/', 'crates/ponko_mcp/tests/')) or name.startswith('examples/cpp/') and not name.endswith('CMakeLists.txt'):
        return 'deprecated compatibility', 'Tests old native symbols/headers or old persisted/environment names during the deprecation cycle.'
    return 'deprecated compatibility', 'Compatibility-layer generation/checks, legacy build fallbacks or preserved leak-check configuration.'


def check(current, baseline):
    errors = []
    for name, counts in current.items():
        allowed = baseline.get(name, {})
        for key, count in counts.items():
            if count > allowed.get(key, 0):
                errors.append(f'{name}: {key} {count} exceeds reviewed budget {allowed.get(key, 0)}')
    return errors


# --paths: one public path per item.

# Path prefixes (below the crate) that may repeat an item: the prelude.
REPEATING_PREFIXES = ('prelude::',)
# The external crate whose items have one way in, and that way.
ECS_CRATE = 'ponko_ecs'
ECS_PATH = 'ecs_core'
# Reviewed exceptions: {path: reason}. Keep this empty if you can.
ALLOWED = {}


def public_paths(doc):
    """{item key: [path, ...]} for every public item reachable from the root.

    Local items are keyed by their id; items of other crates by
    `crate::path` (their id is not in the index). A glob of another crate is
    recorded under the key `<glob crate>`.
    """
    index, ext = doc['index'], doc.get('paths', {})
    crates = doc.get('external_crates', {})
    found = {}

    def note(key, path):
        found.setdefault(key, [])
        if path not in found[key]:
            found[key].append(path)

    def external(target):
        info = ext.get(str(target))
        if not info:
            return f'<unknown {target}>'
        return '::'.join(info['path'])

    def walk(module_id, prefix, seen):
        if module_id in seen:
            return
        seen = seen | {module_id}
        for child in index[module_id]['inner']['module']['items']:
            item = index.get(str(child))
            if item is None:
                continue
            inner = item['inner']
            if 'use' in inner:
                use = inner['use']
                target = use.get('id')
                if use['is_glob']:
                    if target is not None and str(target) in index:
                        walk(str(target), prefix, seen)
                    else:
                        note(f'<glob {use["source"]}>', prefix.rstrip(':'))
                    continue
                path = prefix + use['name']
                if target is not None and str(target) in index:
                    resolved = index[str(target)]
                    if 'module' in resolved['inner']:
                        note(str(target), path)
                        walk(str(target), path + '::', seen)
                    else:
                        note(str(target), path)
                else:
                    note(external(target) if target is not None else use['source'], path)
                continue
            if item.get('name') is None:
                continue  # impls
            path = prefix + item['name']
            note(str(child), path)
            if 'module' in inner:
                walk(str(child), path + '::', seen)

    walk(str(doc['root']), '', frozenset())
    return found


def duplicates(doc):
    """Problems: items public at more than one path outside the prelude."""
    index = doc['index']
    problems = []
    for key, paths in sorted(public_paths(doc).items(), key=lambda kv: sorted(kv[1])):
        item = index.get(key)
        if item is not None and item.get('deprecation'):
            continue
        own = [p for p in paths if not p.startswith(REPEATING_PREFIXES) and p not in ALLOWED]
        if key.startswith(ECS_CRATE + '::'):
            stray = [p for p in own if p != ECS_PATH and not p.startswith(ECS_PATH + '::')]
            if stray:
                problems.append(f'{key}: the ECS crate is reached through ponko::{ECS_PATH}, '
                                f'not {", ".join("ponko::" + p for p in stray)}')
            continue
        if len(own) > 1:
            problems.append(f'{key if not key.isdigit() else own[0]}: public at '
                            + ', '.join('ponko::' + p for p in sorted(own)))
    return problems


def build_json():
    sys.path.insert(0, str(ROOT / 'scripts'))
    import importlib
    snapshots = importlib.import_module('api-snapshots')
    meta = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--no-deps', '--format-version', '1'], cwd=ROOT))
    package = next(p for p in meta['packages'] if p['name'] == 'ponko')
    features = package['metadata']['docs']['rs']['features']
    cmd = ['cargo', f'+{snapshots.NIGHTLY}', 'rustdoc', '-p', 'ponko', '--lib',
           '--features', ','.join(features), '--', '-Z', 'unstable-options',
           '--output-format', 'json', '-A', 'warnings']
    env = {**os.environ, 'RUSTFLAGS': '--cap-lints allow', 'RUSTDOCFLAGS': ''}
    subprocess.run(cmd, cwd=ROOT, env=env, check=True)
    target = Path(meta['target_directory'])
    return target / 'doc' / 'ponko.json'


def main_paths():
    if '--json' in sys.argv:
        path = Path(sys.argv[sys.argv.index('--json') + 1])
    else:
        path = build_json()
    problems = duplicates(json.loads(path.read_text()))
    if problems:
        sys.exit('Items with more than one public path (STABILITY.md, "The crate root and '
                 'globs"; scripts/check-naming.py --paths):\n  ' + '\n  '.join(problems))
    print('Every public item of ponko has one path (besides the prelude).')


def selftest():
    def module(items, name=None):
        return {'name': name, 'inner': {'module': {'items': items}}}

    def use(name, target, glob=False, source=''):
        return {'name': None, 'inner': {'use': {'name': name, 'id': target, 'is_glob': glob,
                                                'source': source}}}
    struct = {'name': 'Thing', 'inner': {'struct': {}}}
    doc = {
        'root': 0,
        'index': {
            '0': module([1, 3, 5, 7, 9], 'ponko'),
            '1': module([2], 'topic'), '2': struct,
            '3': module([4], 'prelude'), '4': use('Thing', 2),
            '5': module([6], 'ecs_core'), '6': use('*', None, True, 'ponko_ecs'),
            '7': module([8], 'other'), '8': use('World', 100),
            '9': module([10], 'old'), '10': {'name': 'Old', 'deprecation': {'since': '0.2.0'},
                                              'inner': {'type_alias': {}}},
        },
        'paths': {'100': {'crate_id': 1, 'path': ['ponko_ecs', 'World']}},
    }
    problems = duplicates(doc)
    assert problems == ['ponko_ecs::World: the ECS crate is reached through ponko::ecs_core, '
                        'not ponko::other::World'], problems
    doc['index']['7'] = module([11], 'other')
    doc['index']['11'] = use('Renamed', 2)
    problems = duplicates(doc)
    assert problems == ['topic::Thing: public at ponko::other::Renamed, ponko::topic::Thing'], problems
    doc['index']['7'] = module([], 'other')
    assert duplicates(doc) == [], duplicates(doc)
    print('check-naming.py selftest passed.')


def main():
    if '--selftest' in sys.argv:
        selftest()
        return
    if '--paths' in sys.argv:
        main_paths()
        return
    current = references()
    if '--inventory' in sys.argv:
        entries = {}
        for name, counts in current.items():
            category, reason = classify(name)
            entries[name] = dict(counts, category=category, reason=reason)
        INVENTORY.write_text(json.dumps({'description': 'Reviewed legacy references; see book/src/naming.md. Decreases are allowed; increases require review.', 'files': entries}, indent=2) + '\n')
        return
    errors = check(current, json.loads(INVENTORY.read_text())['files'])
    if errors:
        sys.exit('\n'.join(errors))
    print(f'Naming inventory passed ({len(current)} legacy-bearing files).')


if __name__ == '__main__':
    main()
