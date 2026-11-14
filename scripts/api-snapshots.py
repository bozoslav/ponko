#!/usr/bin/env python3
"""Keep a listing of every published crate's public API in `api/`.

    api-snapshots.py update    # regenerate api/: run this after changing public API
    api-snapshots.py check     # what CI does: regenerate, fail with a diff if api/ differs
    api-snapshots.py pins      # print TOOL_VERSION=... and NIGHTLY=... (CI reads them)
    api-snapshots.py selftest  # check the planning and diffing with made-up input

A pull request can add, rename or widen public items without anyone noticing
unless there is a list to diff. `cargo public-api` writes one item per line
(`pub fn ponko_ecs::World::spawn(&mut self) -> ponko_ecs::Entity`), sorted, so
a changed API shows up as a few changed lines in the pull request.

Which files exist comes from Cargo (like scripts/semver-checks.py): every
package that can be published (not `publish = false`) with a library API (not
a proc-macro crate, not a binary-only crate), in these feature sets:

  default               the crate's default features
  no-default-features   `--no-default-features` (for `ponko`: headless, no window)
  docs-rs               the features docs.rs builds (`[package.metadata.docs.rs]`):
                        everything public, not `hotpatch` or `bench-internals`
  <extra>               EXTRA_SETS below: `ponko` with `3d` on top of the defaults

A set that lists exactly what `default` lists is not written (a missing file
means "same as default": `ponko` without its window has the same API, which is
the promise of the feature docs). If it starts to differ, its file appears in
the diff. The files are `api/<crate>/<set>.txt`. The listing leaves
out blanket and auto-trait impls (`-ss`): they are noise, and `Send`/`Sync`
changes show up in the compiler. Derived impls (`Clone`, `Debug`, ...) stay.

The tool and the nightly toolchain it needs are pinned together below, because
the tool reads rustdoc's JSON, whose format changes with every nightly. To
move to a newer nightly, move TOOL_VERSION to a release that supports it (its
README has the table), run `update`, and check that the listing changes only
where the tool's changelog says it should. The listing is made for the Linux
docs.rs target: x86_64 Linux. The listing differs by platform in two ways
(types such as `glam::Quat` live in `glam::f32::sse2` on x86_64 and in
`glam::f32::neon` on Apple silicon; a few items exist only on one OS), so on
macOS the script builds for `x86_64-apple-darwin` (same CPU, no Linux system
libraries needed), removes the macOS-only items, MACOS_ONLY_ITEMS, and puts
the Linux-only ones, LINUX_ONLY_ITEMS, in their place. On any
other host it stops and says to use the files CI uploads. The result is the
same text as on the Linux runner; CI has the last word. A new public item
behind `cfg(target_os = ...)` shows up as a diff in CI: add it to
MACOS_ONLY_ITEMS (or take the regenerated files from the CI run).

Relation to scripts/semver-checks.py: the snapshot shows *every* change to the
public API, growth included, so a reviewer decides whether the surface is meant.
The semver job shows only what breaks code that compiled against the last
release. CONTRIBUTING.md has a section on both.
"""
import difflib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
API = ROOT / 'api'

# Pinned together: this tool release reads the rustdoc JSON of this nightly
# (tool README, "Compatible nightly").
TOOL_VERSION = '0.52.0'
NIGHTLY = 'nightly-2026-11-06'

LIB_KINDS = {'lib', 'rlib', 'dylib', 'cdylib', 'staticlib'}
# -ss: no blanket impls, no auto-trait impls.
OMIT_FLAGS = ['-ss']
# Feature sets of a crate beyond the three that come from Cargo:
# {crate: {set name: extra `cargo public-api` flags}}.
EXTRA_SETS = {'ponko': {'3d': ['--features', '3d']}}
# Public items that exist only on macOS: they are not in the Linux listing, so
# the macOS run drops their lines (a line starts with the item, so a prefix of
# its path is enough).
MACOS_ONLY_ITEMS = ['pub unsafe fn ponko::ui::UiAccessAdapter::for_ns_view(']
# Public items that exist only on Linux, which the macOS run can't list: each
# macOS-only item above (by its prefix) is replaced by these lines, where the
# Linux listing has them (the items of one impl are sorted by name, and these
# sort next to the macOS item they replace).
LINUX_ONLY_ITEMS = {
    'pub unsafe fn ponko::ui::UiAccessAdapter::for_ns_view(': ['pub fn ponko::ui::UiAccessAdapter::for_atspi(&str) -> Self'],
}
# How many lines of one file's diff the job summary shows.
SUMMARY_DIFF_LINES = 300

# The macOS host builds for the same CPU as the Linux runner.
MACOS_TARGET = 'x86_64-apple-darwin'

UPDATE_COMMAND = 'python3 scripts/api-snapshots.py update'


# Planning and comparing: plain functions of their input, checked by `selftest`.

def feature_sets(package):
    """{set name: cargo flags} for the package, `default` first."""
    sets = {'default': []}
    name = package['name']
    features = package.get('features') or {}
    if features.get('default'):
        sets['no-default-features'] = ['--no-default-features']
    docs = ((package.get('metadata') or {}).get('docs') or {}).get('rs') or {}
    if docs.get('all-features'):
        sets['docs-rs'] = ['--all-features']
    elif docs.get('features'):
        flags = ['--no-default-features'] if docs.get('no-default-features') else []
        sets['docs-rs'] = flags + ['--features', ','.join(docs['features'])]
    for extra, flags in EXTRA_SETS.get(name, {}).items():
        sets[extra] = flags
    return sets


def skip_reason(package):
    """Why the crate has no listing, or None."""
    kinds = {kind for target in package['targets'] for kind in target['kind']}
    if 'proc-macro' in kinds:
        return 'proc-macro crate (no API to list)'
    if not kinds & LIB_KINDS:
        return 'no library target (binary only, no API)'
    return None


def host_plan(system, machine):
    """(extra cargo flags, line prefixes to drop, tool problem or None) for the host."""
    if system == 'Linux' and machine in ('x86_64', 'AMD64'):
        return [], [], None
    if system == 'Darwin':
        return ['--target', MACOS_TARGET], MACOS_ONLY_ITEMS, None
    return [], [], (f'The listing is made on x86_64 Linux or macOS, not on {system} {machine}. '
                    'Take the regenerated files from the "api-regenerated" artifact of the '
                    'failed CI run (API snapshots job) and copy them into api/.')


def drop_lines(text, prefixes, replacements=None):
    """`text` without the lines that start with one of the prefixes; a dropped
    line whose prefix is a key of `replacements` gives way to its lines."""
    if not prefixes:
        return text
    keep = []
    for line in text.splitlines(keepends=True):
        prefix = next((p for p in prefixes if line.startswith(p)), None)
        if prefix is None:
            keep.append(line)
        else:
            keep.extend(f'{l}\n' for l in (replacements or {}).get(prefix, []))
    return ''.join(keep)


def drop_same_as_default(listings):
    """The listings of one crate without the sets that equal `default`."""
    return {name: text for name, text in listings.items()
            if name == 'default' or text != listings['default']}


def relative(crate, set_name):
    return f'{crate}/{set_name}.txt'


def diff_files(expected, actual):
    """Compare {relative path: text} dicts (committed, regenerated).

    Returns a list of (path, kind, unified diff lines), kind being 'changed',
    'new' (to be added) or 'stale' (to be deleted).
    """
    out = []
    for path in sorted(set(expected) | set(actual)):
        old, new = expected.get(path), actual.get(path)
        if old == new:
            continue
        kind = 'new' if old is None else 'stale' if new is None else 'changed'
        lines = list(difflib.unified_diff(
            (old or '').splitlines(), (new or '').splitlines(),
            fromfile=f'a/api/{path}', tofile=f'b/api/{path}', lineterm='', n=2))
        out.append((path, kind, lines))
    return out


def failure_message(changes):
    """The text a failing check prints (and puts in the job summary)."""
    files = ', '.join(f'`api/{path}` ({kind})' for path, kind, _ in changes)
    return [
        'The public API of this change differs from the snapshot in `api/`.',
        f'Differs: {files}.',
        '',
        'If the change to the public API is meant, regenerate the snapshot and commit it:',
        '',
        f'    {UPDATE_COMMAND}',
        '    git add api',
        '',
        'The diff of `api/` is then the list of public items this pull request adds, '
        'changes or removes: check it is what you intended. If it is not, the change '
        'made something public by accident (`pub` instead of `pub(crate)`, a `pub use` '
        'that pulls in more than it should).',
        'Tool and nightly are pinned in scripts/api-snapshots.py. CONTRIBUTING.md, '
        '"API snapshots", explains how this relates to the Semver checks job.',
    ]


def summary_text(changes):
    out = ['## API snapshots', '', *failure_message(changes), '']
    for path, _, lines in changes:
        shown = lines[:SUMMARY_DIFF_LINES]
        out += [f'### `api/{path}`', '', '```diff', *shown]
        if len(lines) > len(shown):
            out.append(f'... {len(lines) - len(shown)} more lines (run the command above to see them)')
        out += ['```', '']
    return '\n'.join(out) + '\n'


# Side effects: cargo and the file system.

def run(cmd, **kw):
    return subprocess.run(cmd, cwd=ROOT, text=True, **kw)


def packages():
    out = run(['cargo', 'metadata', '--no-deps', '--format-version', '1'],
              capture_output=True, check=True).stdout
    # `publish = false` shows up as `publish: []`.
    return sorted((p for p in json.loads(out)['packages'] if p['publish'] != []),
                  key=lambda p: p['name'])


def group(title):
    print(f'::group::{title}' if os.environ.get('GITHUB_ACTIONS') else f'--- {title}', flush=True)


def endgroup():
    if os.environ.get('GITHUB_ACTIONS'):
        print('::endgroup::', flush=True)


def require_tools():
    """Stop with the exact install commands if the pinned tools are missing."""
    problems = []
    _, _, host_problem = host_plan(platform.system(), platform.machine())
    if host_problem:
        sys.exit(host_problem)
    done = run(['cargo', 'public-api', '--version'], capture_output=True)
    version = done.stdout.split()[-1] if done.returncode == 0 and done.stdout.split() else None
    if version != TOOL_VERSION:
        problems.append(f'cargo-public-api {TOOL_VERSION} is needed (found: {version or "none"}):\n'
                        f'    cargo install --locked cargo-public-api --version {TOOL_VERSION}')
    if run(['rustc', f'+{NIGHTLY}', '--version'], capture_output=True).returncode != 0:
        problems.append(f'The {NIGHTLY} toolchain is needed (the tool builds rustdoc JSON with it; '
                        f'it does not become your default):\n'
                        f'    rustup toolchain install {NIGHTLY} --profile minimal')
    elif platform.system() == 'Darwin':
        installed = run(['rustup', 'target', 'list', '--installed', '--toolchain', NIGHTLY],
                        capture_output=True).stdout.split()
        if MACOS_TARGET not in installed:
            problems.append(f'The {MACOS_TARGET} standard library is needed on macOS (the listing '
                            f'is made for x86_64, like CI):\n'
                            f'    rustup target add {MACOS_TARGET} --toolchain {NIGHTLY}')
    if problems:
        sys.exit('\n\n'.join(problems))


def generate(package, set_name, flags):
    host_flags, drop, _ = host_plan(platform.system(), platform.machine())
    cmd = ['cargo', f'+{NIGHTLY}', 'public-api', '-p', package['name'], *OMIT_FLAGS,
           '--color', 'never', *host_flags, *flags]
    group(f'{package["name"]}: {set_name} ({" ".join(flags) or "default features"})')
    # No `-D warnings` from the environment: the nightly has lints the stable
    # toolchain does not, and the listing does not depend on warnings. Their
    # output would also bury the diff in the log (other jobs lint this code).
    env = {**os.environ, 'RUSTFLAGS': '--cap-lints allow', 'RUSTDOCFLAGS': '-A warnings'}
    done = run(cmd, stdout=subprocess.PIPE, env=env)
    endgroup()
    if done.returncode != 0:
        sys.exit(f'`{" ".join(cmd)}` failed ({done.returncode}); see the output above.')
    if not done.stdout.strip():
        sys.exit(f'`{" ".join(cmd)}` listed nothing: a crate with a library has some public API.')
    text = drop_lines(done.stdout, drop, LINUX_ONLY_ITEMS)
    return text if text.endswith('\n') else text + '\n'


def generate_all():
    """{relative path: text} for every crate and feature set."""
    require_tools()
    files = {}
    for package in packages():
        reason = skip_reason(package)
        if reason:
            print(f'{package["name"]}: not listed, {reason}')
            continue
        listings = {name: generate(package, name, flags)
                    for name, flags in feature_sets(package).items()}
        for name, text in drop_same_as_default(listings).items():
            files[relative(package['name'], name)] = text
    return files


def committed():
    """{relative path: text} of the files in api/ now."""
    if not API.is_dir():
        return {}
    return {str(p.relative_to(API)): p.read_text() for p in sorted(API.glob('*/*.txt'))}


def main_update():
    files = generate_all()
    old = committed()
    for path in old.keys() - files.keys():
        (API / path).unlink()
    for path, text in files.items():
        target = API / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    for crate_dir in API.glob('*'):
        if crate_dir.is_dir() and not any(crate_dir.iterdir()):
            crate_dir.rmdir()
    changes = diff_files(old, files)
    if not changes:
        print('api/ is up to date: the public API did not change.')
    else:
        added = sum(1 for _, _, lines in changes for l in lines if l.startswith('+') and not l.startswith('+++'))
        removed = sum(1 for _, _, lines in changes for l in lines if l.startswith('-') and not l.startswith('---'))
        print(f'api/ updated: {len(changes)} file(s), +{added} -{removed} lines. '
              'Review `git diff api` and commit it.')


def main_check():
    files = generate_all()
    changes = diff_files(committed(), files)
    if not changes:
        print('api/ matches the public API.')
        return 0
    for line in failure_message(changes):
        print(line)
    print()
    for _, _, lines in changes:
        print('\n'.join(lines))
    print()
    ci = os.environ.get('GITHUB_ACTIONS')
    if ci:
        print(f'::error title=Public API differs from api/::Run `{UPDATE_COMMAND}` and commit api/.')
    path = os.environ.get('GITHUB_STEP_SUMMARY')
    if path:
        with open(path, 'a') as f:
            f.write(summary_text(changes))
    # The regenerated files, for the job to upload: a contributor who cannot
    # run the tool (Windows, say) takes them from the run's artifacts.
    out = os.environ.get('API_SNAPSHOTS_REGENERATED')
    if out:
        for rel, text in files.items():
            target = Path(out) / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
    return 1


def selftest():
    def pkg(name, features=None, docs=None, kinds=('lib',)):
        return {'name': name, 'features': features or {}, 'targets': [{'kind': list(kinds)}],
                'metadata': {'docs': {'rs': docs}} if docs else None}

    assert list(feature_sets(pkg('plain'))) == ['default']
    both = feature_sets(pkg('x', {'default': ['a'], 'a': []}, {'all-features': True}))
    assert both == {'default': [], 'no-default-features': ['--no-default-features'],
                    'docs-rs': ['--all-features']}, both
    listed = feature_sets(pkg('y', {'a': []}, {'features': ['a', 'b']}))
    assert listed['docs-rs'] == ['--features', 'a,b'] and 'no-default-features' not in listed
    minimal = feature_sets(pkg('z', {}, {'no-default-features': True, 'features': ['a']}))
    assert minimal['docs-rs'] == ['--no-default-features', '--features', 'a']
    assert '3d' in feature_sets(pkg('ponko', {'default': ['w']}))
    assert skip_reason(pkg('m', kinds=('proc-macro',))) and skip_reason(pkg('b', kinds=('bin',)))
    assert skip_reason(pkg('c', kinds=('rlib', 'cdylib'))) is None

    same = {'a/default.txt': 'pub fn a\n'}
    assert diff_files(same, dict(same)) == []
    changes = diff_files(same, {'a/default.txt': 'pub fn a\npub fn b\n', 'a/new.txt': 'pub fn c\n'})
    assert [(p, k) for p, k, _ in changes] == [('a/default.txt', 'changed'), ('a/new.txt', 'new')]
    assert '+pub fn b' in changes[0][2]
    assert diff_files(same, {})[0][1] == 'stale'
    assert drop_same_as_default({'default': 'a', 'x': 'a', 'y': 'b'}) == {'default': 'a', 'y': 'b'}
    message = '\n'.join(failure_message(changes))
    assert UPDATE_COMMAND in message and 'api/a/new.txt' in message
    assert '```diff' in summary_text(changes)
    assert host_plan('Linux', 'x86_64') == ([], [], None)
    flags, drop, problem = host_plan('Darwin', 'arm64')
    assert flags == ['--target', MACOS_TARGET] and drop == MACOS_ONLY_ITEMS and problem is None
    assert host_plan('Windows', 'AMD64')[2] and host_plan('Linux', 'aarch64')[2]
    text = 'pub fn a\n' + MACOS_ONLY_ITEMS[0] + '*mut u8) -> Self\npub fn b\n'
    assert drop_lines(text, MACOS_ONLY_ITEMS) == 'pub fn a\npub fn b\n'
    assert drop_lines(text, []) == text
    assert drop_lines(text, MACOS_ONLY_ITEMS, {MACOS_ONLY_ITEMS[0]: ['pub fn l']}) == 'pub fn a\npub fn l\npub fn b\n'
    print('selftest ok')


def main(argv):
    command = argv[1] if len(argv) == 2 else ''
    if command == 'update':
        main_update()
    elif command == 'check':
        return main_check()
    elif command == 'pins':
        print(f'API_TOOL_VERSION={TOOL_VERSION}')
        print(f'API_NIGHTLY={NIGHTLY}')
    elif command == 'selftest':
        selftest()
    else:
        print(__doc__)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
