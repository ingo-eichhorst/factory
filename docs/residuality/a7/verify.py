#!/usr/bin/env python3
"""Complete read-only A7 artifact verification; not a product resilience test."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parent
project = root.parents[2]
for script, args in [('build.py',['--inventory-only']),('bind_modules.py',['--check']),
                     ('build.py',['--check']),('test_model.py',[]),('addenda/test_s351.py',[])]:
    subprocess.run([sys.executable,'-B',str(root/script),*args],cwd=project,check=True)
print('A7: Quellenfreeze, Nachträge, Modulzuordnung, Abhängigkeiten und Dokumenttests geprüft. Keine Betriebsfreigabe.')
