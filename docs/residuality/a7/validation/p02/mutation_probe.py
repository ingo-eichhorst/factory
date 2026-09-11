#!/usr/bin/env python3
"""Negative control in a disposable ordinary directory, never a Git worktree.

Disable the real restore journal guard ONLY in a copied workspace. Success of
this script means the altered tests failed as expected, not that P02 is solved.
Requires locally installed Rust and cached dependencies; Cargo stays offline.
"""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
PROJECT = ROOT.parents[4]


def verify_sources(project):
    manifest = json.loads((ROOT / 'source-hashes.json').read_text())['sha256']
    for name, expected in manifest.items():
        # Other sessions may develop unrelated test targets. Those are captured
        # in the baseline inventory but are NOT compiled by this command.
        if '/tests/' in name and name not in {
            'crates/factory-recovery/tests/residuality_p02.rs',
            'crates/factory-recovery/tests/restore_common/mod.rs',
        }:
            continue
        actual = hashlib.sha256((project / name).read_bytes()).hexdigest()
        if actual != expected:
            raise SystemExit(f'Source revision changed: {name}; review before rerunning this pinned probe')


def main():
    verify_sources(PROJECT)
    with tempfile.TemporaryDirectory(prefix='factory-p02-mutation-') as directory:
        copy = Path(directory)
        shutil.copytree(PROJECT / 'crates', copy / 'crates',
                        ignore=shutil.ignore_patterns('target', '__pycache__'))
        for name in ['Cargo.toml', 'Cargo.lock']:
            shutil.copyfile(PROJECT / name, copy / name)
        verify_sources(copy)
        source = copy / 'crates/factory-recovery/src/restore.rs'
        original = source.read_text()
        guard = 'if has_attempt {'
        assert original.count(guard) == 1
        source.write_text(original.replace(guard, 'if false && has_attempt {'))
        result = subprocess.run([
            'cargo', 'test', '--offline', '--locked', '--target-dir', str(copy / 'target'),
            '-p', 'factory-recovery', '--test', 'residuality_p02',
        ], cwd=copy, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        print(result.stdout, end='')
        expected_failures = [
            'retained_attempt_blocks_after_receipt_with_lost_ack',
            'retained_attempt_also_blocks_when_writer_received_nothing',
            'resume_increases_budget_without_resolving_ambiguous_receipt',
        ]
        assert result.returncode == 101, 'mutated test binary must fail'
        for name in expected_failures:
            assert f'test {name} ... FAILED' in result.stdout, 'not the expected test failure'
        assert 'test old_snapshot_cannot_distinguish_no_receipt_from_later_receipt ... ok' in result.stdout
        assert '1 passed; 3 failed' in result.stdout
    verify_sources(PROJECT)
    print('EXPECTED NEGATIVE CONTROL: three guard-sensitive tests rejected the mutation; pinned library/manifest/probe sources unchanged.')


if __name__ == '__main__':
    main()
