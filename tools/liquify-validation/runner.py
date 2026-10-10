#!/usr/bin/env python3
"""Run real Rust validation, preserving evidence for every gate.

This tool does not install Rust, claim Python tests are Rust tests, or publish code.
Missing tools are BLOCKED, not PASSED. Failed gates do not hide subsequent results.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent


@dataclass(frozen=True)
class Gate:
    name: str
    command: tuple[str, ...]
    expected_names: tuple[str, ...] = ()
    require_tests: bool = False


def missing_expected(text: str, names: tuple[str, ...]) -> list[str]:
    # libtest discovery is stronger than merely accepting "0 tests passed".
    listed = {line.removesuffix(': test').rsplit('::', 1)[-1]
              for line in text.splitlines() if line.endswith(': test')}
    return sorted(set(names) - listed)


def test_count(text: str) -> int:
    return sum(int(n) for n in re.findall(r'(?m)^test result: ok\. (\d+) passed;', text))


def atomic_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix='.validation-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as stream:
            json.dump(value, stream, indent=2)
            stream.write('\n')
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def terminate(process: subprocess.Popen) -> None:
    if process.poll() is not None:
        return
    # Do not leave rustc/linker descendants running after a timed-out Cargo gate.
    if os.name == 'posix':
        os.killpg(process.pid, signal.SIGTERM)
    else:
        process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        if os.name == 'posix':
            os.killpg(process.pid, signal.SIGKILL)
        else:
            process.kill()
        process.wait()


def run_gate(root: Path, output: Path, gate: Gate, timeout: float) -> dict:
    log = output / f'{gate.name}.txt'
    result = {'name': gate.name, 'command': list(gate.command), 'status': 'running',
              'executed': False, 'log': log.name}
    start = time.monotonic()
    with log.open('w', encoding='utf-8') as stream:
        stream.write('$ ' + ' '.join(gate.command) + '\n')
        stream.flush()
        try:
            with subprocess.Popen(gate.command, cwd=root, stdout=stream,
                                  stderr=subprocess.STDOUT,
                                  start_new_session=(os.name == 'posix'),
                                  env={**os.environ, 'CARGO_TERM_COLOR': 'never'}) as process:
                result['executed'] = True
                try:
                    result['exit_code'] = process.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    terminate(process)
                    result['status'] = 'timed_out'
                    result['error'] = f'Gate exceeded {timeout:g} seconds'
                except KeyboardInterrupt:
                    terminate(process)
                    result['status'] = 'cancelled'
                    result['error'] = 'Interrupted by user'
        except OSError as error:
            result['status'] = 'blocked'
            result['error'] = str(error)
    result['seconds'] = round(time.monotonic() - start, 3)
    text = log.read_text(encoding='utf-8', errors='replace')
    if result['status'] == 'running':
        result['status'] = 'passed' if result['exit_code'] == 0 else 'failed'
        if result['status'] == 'passed' and gate.expected_names:
            missing = missing_expected(text, gate.expected_names)
            if missing:
                result['status'] = 'failed'
                result['error'] = 'Authored tests were not discovered'
                result['missing_tests'] = missing
        if gate.require_tests:
            result['tests_passed'] = test_count(text)
            if result['status'] == 'passed' and result['tests_passed'] == 0:
                result['status'] = 'failed'
                result['error'] = 'No successful test execution found; zero tests is not validation'
    return result


def build_gates(profile: str, expected: dict) -> list[Gate]:
    return [
        Gate('format', ('cargo', 'fmt', '--all', '--', '--check')),
        Gate('algo_discovery', ('cargo', 'test', '--locked', '-p', 'photocraft-algo', '--lib', 'liquify', '--', '--list'), tuple(expected['algo'])),
        Gate('ui_discovery', ('cargo', 'test', '--locked', '-p', 'photocraft-ui-egui', '--lib', 'liquify', '--', '--list'), tuple(expected['ui'])),
        Gate('algo_tests', ('cargo', 'test', '--locked', '-p', 'photocraft-algo', '--lib', 'liquify'), require_tests=True),
        Gate('ui_tests', ('cargo', 'test', '--locked', '-p', 'photocraft-ui-egui', '--lib', 'liquify'), require_tests=True),
        Gate('engine_tests', ('cargo', 'test', '--locked', '-p', 'photocraft-engine', '--lib', 'distort'), require_tests=True),
        Gate('layers', ('cargo', 'xtask', 'layers')),
        Gate('clippy', ('cargo', 'clippy', '--locked', '-p', 'photocraft-algo', '-p', 'photocraft-ui-egui', '-p', 'photocraft-engine', '--all-targets', '--', '-D', 'warnings')),
        Gate('wasm', ('cargo', 'xtask', 'wasm')),
        Gate('panic_input', ('cargo', 'test', '--locked', '-p', 'photocraft-engine', '--test', 'panic_hunt', '--', '--ignored'), require_tests=True),
        Gate('build', ('cargo', 'build', '--locked', '-p', 'photocraft') + (('--release',) if profile == 'release' else ())),
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('checkout', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--profile', choices=('debug', 'release'), default='debug')
    parser.add_argument('--gate-timeout', type=float, default=1800.0)
    parser.add_argument('--format-touched', action='store_true', help='Explicitly format the four touched host modules and their children; retain the diff.')
    args = parser.parse_args()
    root, output = args.checkout.resolve(), args.output.resolve()
    # Evidence must not overwrite work or silently mix runs.
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        parser.error('Output directory must be new or empty; preserve earlier reports')
    output.mkdir(parents=True, exist_ok=True)
    summary = {'status': 'blocked', 'rust_executed': False, 'profile': args.profile,
               'checked_at_utc': datetime.now(timezone.utc).isoformat(),
               'checkout': str(root), 'checkout_complete': (root/'Cargo.toml').is_file(),
               'gates': [], 'limitations': [
                   'Does not run interactive portrait QA or prove Photoshop superiority.',
                   'WASM compilation does not establish native-quality web rendering.',
               ]}
    summary_path = output / 'summary.json'
    missing = [tool for tool in ('git', 'cargo', 'rustc') if shutil.which(tool) is None]
    if missing:
        summary['blocker'] = 'Missing tools: ' + ', '.join(missing)
        atomic_json(summary_path, summary)
        print(summary['blocker'] + '. No Rust gates ran.', file=sys.stderr)
        return 2
    if not (root / 'Cargo.toml').is_file() or args.gate_timeout <= 0:
        summary['blocker'] = 'Need a complete PhotoCraft checkout and a positive gate timeout'
        atomic_json(summary_path, summary)
        return 2
    try:
        for label, command in [('head', ('git', 'rev-parse', 'HEAD')),
                               ('rustc', ('rustc', '--version')),
                               ('cargo', ('cargo', '--version'))]:
            summary[label] = subprocess.check_output(command, cwd=root, text=True, stderr=subprocess.STDOUT).strip()
        expected = json.loads((HERE / 'expected-tests.json').read_text())
        gates = build_gates(args.profile, expected)
        if args.format_touched:
            # The bootstrap creates a dedicated feature checkout. Formatting is
            # explicit and its exact diff is retained, not silently committed.
            modules = ['crates/algo/src/liquify.rs', 'crates/engine/src/distort_cmds.rs',
                       'crates/ui-egui/src/liquify_ui.rs', 'crates/ui-egui/src/smart_ui.rs']
            gates.insert(0, Gate('format_touched', ('rustfmt', '--edition', '2024', *modules)))
        summary['status'] = 'running'
        atomic_json(summary_path, summary)
        for gate in gates:
            print(f'Running {gate.name}: {" ".join(gate.command)}', flush=True)
            result = run_gate(root, output, gate, args.gate_timeout)
            summary['gates'].append(result)
            summary['rust_executed'] |= result['executed'] and gate.command[0] == 'cargo' and gate.command[1] != 'fmt'
            atomic_json(summary_path, summary)
            print(f'  {result["status"]}: {output / result["log"]}', flush=True)
            if result['status'] == 'cancelled':
                break
        # Never use a successful build to conceal a failed test/lint/format gate.
        summary['status'] = 'passed' if len(summary['gates']) == len(gates) and all(g['status'] == 'passed' for g in summary['gates']) else 'failed'
        with (output / 'tested-worktree.patch').open('w') as stream:
            subprocess.run(['git', 'diff', '--no-ext-diff', '--binary', 'HEAD'], cwd=root, stdout=stream, check=True)
        summary['tested_worktree_may_differ_from_commit'] = bool((output / 'tested-worktree.patch').stat().st_size)
        atomic_json(summary_path, summary)
        print(f'Overall: {summary["status"]}. Evidence: {summary_path}')
        return 0 if summary['status'] == 'passed' else 1
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        summary['status'] = 'blocked' if not summary['gates'] else 'failed'
        summary['blocker'] = str(error)
        atomic_json(summary_path, summary)
        print(str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
