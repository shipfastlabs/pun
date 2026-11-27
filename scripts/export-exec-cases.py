#!/usr/bin/env python3
"""Write crates/pun-cli/tests/fixtures/exec.json, the manifest of crates/pun-cli/tests/exec.rs: small projects with
what `composer exec` 2.11.2 (.oracle/composer, the Composer the plugin host pins) prints and returns for them, and a
few `composer run-script` cases that share its way of running a command (a binary of the root's own `bin`).

Each case is a set of files (`executable` lists those that get mode 755), the words after `composer`, and the
directory to run in (`cwd`, the project's by default), run in a fresh temporary directory with a fresh HOME and
COMPOSER_HOME, COMPOSER_ROOT_VERSION=1.0.0 and COLUMNS=500, without a terminal on stdin (so without interaction). The
directory is written `{dir}` in the output. A case records `exit`, `stdout` and `stderr`, and `message` (the lines of
Composer's exception block) when Composer stops with an exception, which Pun prints as its own run error (exit 2).
`stderr_tail` marks a case whose stderr starts with what /bin/sh itself says, which differs between systems: only
Composer's lines after it are checked.

/bin/sh is bash on macOS and dash on Linux, which word their errors and exit codes differently, so the run records the
results of the system it runs on and keeps the other system's from the fixture it replaces: a result both share stays
on the case, and one that differs goes under `macos` and `linux` (null where one has no `message`), which the test
picks from by the system it runs on. Re-record on both systems after a change to the cases; a case the old fixture
lacks takes this system's results for both until then.

usage: scripts/export-exec-cases.py   (needs php 8.5 and .oracle/composer from scripts/oracle.sh)
"""
import json, os, re, shutil, subprocess, sys, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PHAR = os.path.join(ROOT, '.oracle/composer/composer.phar')
OUT = os.path.join(ROOT, 'crates/pun-cli/tests/fixtures/exec.json')


def j(value):
    return json.dumps(value, indent=4) + '\n'


# A binary that says what it got, from where, and in which environment.
TOOL = '#!/bin/sh\necho "tool $#: $*"\necho "dev=$COMPOSER_DEV_MODE"\necho "path=${PATH%%:*}"\necho "pwd=$(pwd)"\n'
FAIL = '#!/bin/sh\necho "failing" >&2\nexit 3\n'
SLOW = '#!/bin/sh\nsleep 3\necho done\n'
LOCAL_SH = '#!/bin/sh\necho "mine $*"\n'
LOCAL_ENV = '#!/usr/bin/env sh\necho "mine env $*"\n'
LOCAL_PHP = '<?php echo "mine php ", implode(" ", array_slice($argv, 1)), "\\n";\n'

ROOT_JSON = {'name': 'acme/app', 'version': '1.0.0'}
# What a case records of Composer's run.
RESULT = ['exit', 'stdout', 'stderr', 'message']


def project(prefix='', bins=None, manifest=None, executable_bins=True):
    """composer.json and the given files of vendor/bin (or another bin dir), under `prefix`."""
    files = {prefix + 'composer.json': j(manifest or ROOT_JSON)}
    executable = []
    for path, content in (bins or {}).items():
        files[prefix + path] = content
        if executable_bins:
            executable.append(prefix + path)
    return files, executable


def case(name, files_executable, args, cwd='', stderr_tail=False):
    files, executable = files_executable
    return {'name': name, 'files': files, 'executable': executable, 'args': args, 'cwd': cwd, 'stderr_tail': stderr_tail}


CASES = [
    # A binary of the bin dir, its arguments, COMPOSER_DEV_MODE, the bin dir first on the PATH; no `> tool` line.
    case('binary', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', 'a', 'b c']),
    case('binary-dashdash', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '--', '--x', 'a b', "it's"]),
    case('binary-unknown-option', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '--x']),
    case('binary-no-args', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool']),
    # The binary is a command line of its own, as Composer gives it to /bin/sh.
    case('binary-with-words', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool x', 'y']),
    case('binary-exit-code', project(bins={'vendor/bin/fail': FAIL}), ['exec', 'fail']),
    case('binary-not-found', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'nope'], stderr_tail=True),
    case('binary-not-executable', project(bins={'vendor/bin/tool': TOOL}, executable_bins=False), ['exec', 'tool'], stderr_tail=True),
    # Global options, before or after the command.
    case('global-before', project(bins={'vendor/bin/tool': TOOL}), ['-n', 'exec', 'tool', 'a']),
    case('global-after', project(bins={'vendor/bin/tool': TOOL}), ['exec', '-n', 'tool', 'a']),
    # -d reads the project there but runs the binary in the directory Composer started in.
    case('quiet', project(bins={'vendor/bin/tool': TOOL}), ['exec', '-q', 'tool', 'a']),
    case('quiet-failing', project(bins={'vendor/bin/fail': FAIL}), ['exec', 'fail', '--quiet']),
    case('verbose', project(bins={'vendor/bin/tool': TOOL}), ['exec', '-v', 'tool', 'a']),
    case('verbose-after', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', 'a', '-vv']),
    case('no-interaction-after-binary', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', 'a', '-n', '--no-interaction', 'b']),
    case('options-after-binary', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', 'a', '--no-plugins', '--no-scripts', '--no-cache', '--no-ansi', 'b']),
    case('list-after-binary', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '-l']),
    case('unknown-short-option', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '-x']),
    case('short-option-cluster', project(bins={'vendor/bin/tool': TOOL}), ['exec', '-nl']),
    case('dash-alone', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '-']),
    case('script-reference', project(bins={'vendor/bin/tool': TOOL}, manifest={**ROOT_JSON, 'scripts': {'go': 'tool from-go'}}), ['exec', '@go', 'x']),
    case('script-reference-verbose', project(bins={'vendor/bin/tool': TOOL}, manifest={**ROOT_JSON, 'scripts': {'go': ['tool from-go', 'Composer\\Config::disableProcessTimeout']}}), ['exec', '-v', '@go', 'x']),
    case('script-reference-quiet', project(bins={'vendor/bin/tool': TOOL}, manifest={**ROOT_JSON, 'scripts': {'go': ['tool from-go', '@nope']}}), ['exec', '-q', '@go']),
    case('working-dir', project('sub/', bins={'vendor/bin/tool': TOOL}), ['-d', 'sub', 'exec', 'tool', 'a']),
    case('working-dir-after', project('sub/', bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '-d', 'sub']),
    case('bin-dir-config', project(bins={'tools/tool': TOOL}, manifest={**ROOT_JSON, 'config': {'bin-dir': 'tools'}}), ['exec', 'tool']),
    # The list: --list, or no binary (without interaction); the bin dir's files, a .bat beside its binary left out, then
    # the root's own `bin`.
    case('list', project(bins={'vendor/bin/tool': TOOL, 'vendor/bin/tool.bat': '@echo off\n', 'vendor/bin/fail': FAIL, 'vendor/bin/other.bat': '@echo off\n'},
                         manifest={**ROOT_JSON, 'bin': ['bin/mine', 'scripts/second.php']}), ['exec', '--list']),
    case('list-ansi', project(bins={'vendor/bin/tool': TOOL}, manifest={**ROOT_JSON, 'bin': ['bin/mine']}), ['exec', '--list', '--ansi']),
    case('failing-ansi', project(bins={'vendor/bin/fail': FAIL}), ['exec', 'fail', '--ansi']),
    case('working-dir-option-value', project('sub/', bins={'vendor/bin/tool': TOOL}), ['exec', '--working-dir=sub', 'tool']),
    case('working-dir-missing-value', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '-d']),
    case('flag-with-value', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '--list=1']),
    case('list-quiet', project(bins={'vendor/bin/tool': TOOL}), ['exec', '--list', '-q']),
    case('working-dir-not-found', project(bins={'vendor/bin/tool': TOOL}), ['exec', 'tool', '-d', 'nope']),
    case('script-reference-verbose-no-args', project(bins={'vendor/bin/tool': TOOL}, manifest={**ROOT_JSON, 'scripts': {'go': 'tool'}}), ['exec', '-v', '@go']),
    case('root-bin-bat', project(bins={'bin/mine.bat': '@echo off\n'}, manifest={**ROOT_JSON, 'bin': ['bin/mine.bat']}), ['exec', 'mine.bat'], stderr_tail=True),
    case('list-short', project(bins={'vendor/bin/tool': TOOL}), ['exec', '-l']),
    # The list compares the root's binaries with ` (local)` appended, so a .bat beside one of them stays.
    case('list-root-bin-bat', project(bins={'bin/mine': LOCAL_SH, 'bin/mine.bat': '@echo off\n'},
                                      manifest={**ROOT_JSON, 'bin': ['bin/mine', 'bin/mine.bat']}), ['exec', '--list']),
    case('list-without-binary', project(bins={'vendor/bin/tool': TOOL}), ['exec']),
    case('list-with-binary', project(bins={'vendor/bin/tool': TOOL}), ['exec', '--list', 'tool']),
    case('list-root-bin-only', project(manifest={**ROOT_JSON, 'bin': 'bin/mine'}), ['exec', '--list']),
    case('list-empty', project(), ['exec', '--list']),
    case('list-empty-bin-dir-config', project(manifest={**ROOT_JSON, 'config': {'bin-dir': 'tools'}}), ['exec']),
    # A binary of the root's own `bin` runs through the interpreter its shebang names, else php.
    case('root-bin-sh', project(bins={'bin/mine': LOCAL_SH}, manifest={**ROOT_JSON, 'bin': ['bin/mine']}, executable_bins=False), ['exec', 'mine', 'a']),
    case('root-bin-env', project(bins={'bin/mine': LOCAL_ENV}, manifest={**ROOT_JSON, 'bin': ['bin/mine']}, executable_bins=False), ['exec', 'mine', 'a']),
    case('root-bin-php', project(bins={'bin/mine.php': LOCAL_PHP}, manifest={**ROOT_JSON, 'bin': ['bin/mine.php']}, executable_bins=False), ['exec', 'mine.php', 'a']),
    case('root-bin-string', project(bins={'bin/mine': LOCAL_SH}, manifest={**ROOT_JSON, 'bin': 'bin/mine'}, executable_bins=False), ['exec', 'mine', 'a']),
    case('root-bin-by-path', project(bins={'bin/mine': LOCAL_SH}, manifest={**ROOT_JSON, 'bin': ['bin/mine']}, executable_bins=False), ['exec', 'bin/mine', 'a']),
    # The same holds for a script's command that is a root binary on its own.
    case('run-script-root-bin', project(bins={'bin/mine': LOCAL_SH}, manifest={**ROOT_JSON, 'bin': ['bin/mine'], 'scripts': {'go': 'mine'}}, executable_bins=False), ['run-script', 'go', '--', 'x']),
    case('run-script-root-bin-with-words', project(bins={'bin/mine': LOCAL_SH + 'exit 0\n'}, manifest={**ROOT_JSON, 'bin': ['bin/mine'], 'scripts': {'go': 'mine y'}}), ['run-script', 'go'], stderr_tail=True),
    # Composer's process timeout.
    case('timeout', project(bins={'vendor/bin/slow': SLOW}, manifest={**ROOT_JSON, 'config': {'process-timeout': 1}}), ['exec', 'slow']),
    # No composer.json.
    case('no-composer-json', ({}, []), ['exec', 'tool']),
]


def composer(cwd, args, home):
    env = {'PATH': os.environ['PATH'], 'HOME': home, 'COMPOSER_HOME': os.path.join(home, '.composer'),
           'COMPOSER_CACHE_DIR': os.path.join(home, '.cache'), 'COMPOSER_ROOT_VERSION': '1.0.0', 'COLUMNS': '500'}
    return subprocess.run(['php', PHAR, *args], cwd=cwd, env=env, capture_output=True, text=True, stdin=subprocess.DEVNULL,
                          timeout=60)


def exception_message(stderr):
    """The lines of Composer's exception block (`In X line N:` and the message), without the command's usage."""
    lines = stderr.splitlines()
    start = next((i for i, l in enumerate(lines) if re.match(r'^In \S+ line \d+:$', l)), None)
    if start is None:
        return None
    message = []
    for line in lines[start + 1:]:
        if line.startswith('exec [') or line.startswith('run-script [') or line.startswith('Exception trace:'):
            break
        text = line[2:].rstrip() if line.startswith('  ') else line.rstrip()
        if re.match(r'^In \S+ line \d+:$', text):
            continue
        if text and not re.fullmatch(r'\[[\w\\]+\]', text):
            message.append(text)
    return message


def record(c):
    home = tempfile.mkdtemp(prefix='pun-exec-home-')
    work = tempfile.mkdtemp(prefix='pun-exec-')
    try:
        for path, content in c['files'].items():
            file = os.path.join(work, path)
            os.makedirs(os.path.dirname(file), exist_ok=True)
            with open(file, 'w') as f:
                f.write(content)
        for path in c['executable']:
            os.chmod(os.path.join(work, path), 0o755)
        r = composer(os.path.join(work, c['cwd']), c['args'], home)
        here = os.path.realpath(work)
        out = {**c, 'exit': r.returncode, 'stdout': r.stdout.replace(here, '{dir}').replace(work, '{dir}'),
               'stderr': r.stderr.replace(here, '{dir}').replace(work, '{dir}')}
        message = exception_message(out['stderr'])
        if message is None and r.returncode == 1 and re.search(r'\n\n(?:exec|run-script) \[', out['stderr']):
            # Symfony's own error for the command line, before the command's usage.
            message = [l.strip() for l in out['stderr'].split('\n\n')[0].splitlines() if l.strip()]
        if 'Composer could not find' in out['stderr']:
            # Composer prints this error itself, outside an exception block; it is still a run error.
            message = [line for line in out['stderr'].splitlines() if line]
        if message is not None:
            # The test reads only these lines of a run error.
            out['message'] = message
            out['stderr'] = ''
        assert '/var/folders' not in json.dumps(out) and home not in json.dumps(out), (c['name'], out)
        return out
    finally:
        shutil.rmtree(home)
        shutil.rmtree(work)


def main():
    version = subprocess.run(['php', PHAR, '--version'], capture_output=True, text=True).stdout
    assert 'Composer version 2.11.2 ' in version, version
    this = {'darwin': 'macos', 'linux': 'linux'}[sys.platform]
    other = {'macos': 'linux', 'linux': 'macos'}[this]
    try:
        with open(OUT) as f:
            old = {c['name']: c for c in json.load(f)['cases']}
    except FileNotFoundError:
        old = {}
    cases = []
    for c in CASES:
        out = record(c)
        mine = {k: out.pop(k, None) for k in RESULT}
        if c['name'] in old:
            before = old[c['name']]
            theirs = {k: before.get(other, {}).get(k, before.get(k)) for k in RESULT}
        else:
            print(f'{c["name"]}: no {other} results yet, re-record there')
            theirs = mine
        out.update({k: v for k, v in mine.items() if v == theirs[k] and v is not None})
        split = [k for k in RESULT if mine[k] != theirs[k]]
        if split:
            out['linux'], out['macos'] = ({k: r[k] for k in split} for r in ((mine, theirs) if this == 'linux' else (theirs, mine)))
        cases.append(out)
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, 'w') as f:
        json.dump({'cases': cases}, f, indent=1, ensure_ascii=False)
        f.write('\n')
    print(f'{OUT}: {len(cases)} cases')


if __name__ == '__main__':
    main()
