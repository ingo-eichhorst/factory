# A7-Gegenreview: Prüfprotokoll

Run: `c5932c99-b970-464b-a22c-6344a3b4f468`  
Datum: 2026-09-09. Alle hier genannten Prüfungen sind lokal und ohne externe Wirkungen erfolgt.

## Vollständig gelesener Bestand

- `reviews/REVIEW-PROTOCOL.md` und ergänzend `reviews/A7-PROTOCOL.md`.
- Für **operations, governance, observability und boundaries** jeweils vollständig `a7/report.md`, `a7/residues.csv` und `a7/cases.json`.
- `docs/residuality/a7/zielarchitektur.md`, `modules.csv`, `module-bindings.csv`, `module-dependencies.csv`, `build.py` und `discovery-freeze.json` vollständig.
- Ergänzend `bind_modules.py`, `test_model.py` und `pruefplan.md`, um Generierung, vorhandene Dependencytests und die ausdrücklich noch ausstehenden Wirkungsprüfungen korrekt einzuordnen.
- `generated/coverage.csv` vollständig geparst und jede Zeile in sämtlichen sieben Feldern mit den Quellen verglichen. Die bereits gelesenen Falltexte wurden nicht nochmals als generiertes Megadokument gelesen.

Die Quellfälle wurden mit dem jeweiligen vollständigen Szenariotext aus `scenarios.csv` ausgegeben. Die kompakte Ausgabe enthielt unverändert sämtliche Werte: Stressor-ID, historische Quellzustände, Zustand, Art, Status, Residue-IDs, Bedingungen, Fortgang, Begründung, Gegenprüfung und Architekturfolge. Nur JSON-Einrückung und lange Feldnamen wurden verkürzt. Es gab keine Auswahl nur interessanter Zweige und keine Textkürzung innerhalb eines Felds.

Vollständige Leseportionen:

| Bereich | Portionen | Fälle | Zweige | Kandidaten |
|---|---|---:|---:|---:|
| operations | S001–S015, S016–S035, S036–S055, S056–S075, S076–S100 | 100 | 273 | 33 |
| governance | S101–S125, S126–S150, S151–S175, S176–S200 | 100 | 214 | 38 |
| observability | S201–S225, S226–S250, S251–S275 | 75 | 171 | 42 |
| boundaries | S276–S300, S301–S325, S326–S350 | 75 | 194 | 39 |
| **Summe** | | **350** | **852** | **152** |

Historische Zustandsdateien wurden außerdem von den vorhandenen Prüfprogrammen für Referenz-/Hashprüfungen eingelesen. Es erfolgte keine neue Prüfung der Produktionsimplementierung. Historische Sourceaussagen in den Abgaben bleiben deren enger, ausdrücklich nicht aktualisierter Quellenbezug.

## Ausgeführte Prüfungen

Aus `reviews/skeptic/`:

```sh
python3 -B ../../docs/residuality/a7/build.py --check
python3 -B ../../docs/residuality/a7/test_model.py
python3 -B ../../docs/residuality/a7/bind_modules.py --check
```

Ergebnisse:

```text
A7 artifacts checked
Ran 17 tests ... OK
Explicit module assignments checked
FREEZE: 12/12 matches
COVERAGE: all 350 rows / all 7 fields exact
COUNTS: 350 cases, 852 branches, 152 candidates, 16 modules
DEPENDENCIES: 27; bootstrap 9, command 15, optional 3
BOOTSTRAP: acyclic
BINDING DISTINCT REASONS: 23
```

Zusätzlich unabhängig geprüft:

- Alle zwölf SHA-256-Werte aus `discovery-freeze.json` stimmen mit den realen A7-Originaldateien überein.
- 350 Coveragezeilen, jeweils identisch in `stressor_id`, `scenario`, `reviewer`, `branch_ids`, `residue_ids`, `dispositions`, `architectural_consequence`.
- Alle 27 Abhängigkeitskanten haben existierende Module als Endpunkte; der Graph der neun Bootstrapkanten ist azyklisch.
- 152 Bindungen für 152 Kandidaten, jeweils ein primärer Vertragseigner. Keine automatische Zusammenlegung von Kandidaten festgestellt. Die meisten Unterstützungslisten/Begründungen kommen allerdings aus einer gemeinsamen Vorlage pro primärem Modul; das erklärt die konkreten Auflagen A7S05–A7S07, beweist sie nicht allein.

### Dateistatistik der Zweige

| Art | Anzahl |
|---|---:|
| extern-erzwungen | 85 |
| halt | 216 |
| verlust | 128 |
| ungewissheit | 113 |
| transient | 97 |
| attraktorhypothese | 65 |
| offen | 27 |
| abschluss | 112 |
| eskalation | 9 |

| Residue-Status | Anzahl |
|---|---:|
| bedingt | 398 |
| teilweise | 304 |
| keines | 102 |
| ungeklaert | 48 |

Dies sind ausschließlich gezählte Etiketten. Sie sind keine Zahl verschiedener Regime, keine globale Attraktormenge und keine Quote tatsächlich überlebender Systeme.

## Zwei negative Dokumentproben, keine Produktversuche

Alle Änderungen erfolgten ausschließlich an Pythonobjekten im Speicher. `-B` verhinderte Python-Bytecodeausgaben. Es wurden weder eingefrorene Dateien noch generierte Architekturdateien geändert.

### 1. Zusätzlicher Bootstrapzyklus

Über `unittest.mock.patch` wurde `build.read_csv` so ersetzt, dass beim Lesen von `module-dependencies.csv` eine zusätzliche Kante M07→M01 als `bootstrap` geliefert würde. Die bestehende Kante M01→M07 schließt dann einen Zyklus.

- `build.render()` lieferte unverändert dieselben Dateien wie vorher.
- Die instrumentierte Leseliste zeigte: `module-dependencies.csv` wurde von `render()` überhaupt nicht angefragt.
- Derselbe Patch beim vorhandenen Test `test_boot_dependencies_have_no_cycle` erzeugte genau einen erwarteten Testfehlschlag, keinen Laufzeitfehler.

**Schluss:** Der Buildcheck und der separate Dependencytest haben unterschiedliche Reichweiten. Der Dependencytest funktioniert für diesen negativen Fall. Es wäre falsch zu behaupten, A7 habe überhaupt keine Zyklusprüfung. Keine Aussage über tatsächliche Parser-, Lock- oder Startabhängigkeiten eines Produkts folgt daraus.

### 2. Umordnung eingefrorener Zweige

Der JSON-Parser wurde im Speicher für S156 so verändert, dass seine beiden Zweige vertauscht werden. `build.render()` akzeptierte diese Reihenfolge und gab aus:

```text
S156.B01 = Companyinhalte tatsächlich veröffentlicht und gelesen
```

Im unveränderten Original bedeutet S156.B01 dagegen „Rootexport vor Sendung als falsch erkannt“.

**Schluss:** Branch-IDs sind Positionskennungen. Der Compiler erzwingt selbst keinen Vergleich mit dem A7-Discovery-Freeze. Er kann nach einer Umordnung eine bestehende Reviewreferenz anders belegen. Diese Probe ist kein Beleg einer tatsächlich vorgenommenen Quellenänderung. Die echten Dateien stimmen weiterhin mit dem Freeze überein.

Kompakter reproduzierbarer Kern beider Proben, vom Projektroot mit `python3 -B` auszuführen; er schreibt keine Dateien:

```python
import json, sys, io, csv, unittest
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0, str(Path('docs/residuality/a7').resolve()))
import build, test_model

baseline = build.render()
read_csv = build.read_csv
seen = []
def cyclic(path, *args, **kwargs):
    seen.append(path.name)
    rows = read_csv(path, *args, **kwargs)
    if path.name == 'module-dependencies.csv':
        rows.append(dict(source='M07', target='M01', phase='bootstrap',
                         contract='synthetic cycle', failure_response='reject'))
    return rows
with patch.object(build, 'read_csv', side_effect=cyclic):
    assert build.render() == baseline
    assert 'module-dependencies.csv' not in seen
    result = unittest.TestResult()
    test_model.A7Integrity('test_boot_dependencies_have_no_cycle').run(result)
    assert len(result.failures) == 1 and not result.errors

loads = json.loads
def reordered(text, *args, **kwargs):
    value = loads(text, *args, **kwargs)
    if (isinstance(value, list) and value and isinstance(value[0], dict)
            and value[0].get('stressor_id') == 'S101'):
        next(c for c in value if c['stressor_id'] == 'S156')['branches'].reverse()
    return value
with patch.object(build.json, 'loads', side_effect=reordered):
    rows = list(csv.DictReader(io.StringIO(build.render()['branches.csv'])))
    assert next(r for r in rows if r['branch_id'] == 'S156.B01')['state'] == \
        'Companyinhalte tatsächlich veröffentlicht und gelesen'
```

## Identität des geprüften Architekturstands

Die zwölf Abgaben sind über den vollständigen unten identifizierten Discovery-Freeze gebunden. Weitere geprüfte Hauptinputs:

| Datei unter `docs/residuality/a7/` | SHA-256 |
|---|---|
| `discovery-freeze.json` | `b70f2f068b525e15a323d5fc46accfe1fad5967a00090cc76c0c7e89a68a8ff9` |
| `zielarchitektur.md` | `14c1d9332ca9d13a038c4b6d590b271d45bcda8fd4434c3db036f5385bc9a210` |
| `modules.csv` | `4eb6f87dd0ee9906afdcd2350ef4799047fda4fe756b05e39f663256ed1dcd25` |
| `module-bindings.csv` | `88ef3bc014b712ed8d3e414708c15d2342a9ee8dd7b62bed46cb6f5c35adeb83` |
| `module-dependencies.csv` | `4a79e8dd014475ea309295418aef34941304162b7fa6c8325fbd6fcc1777c49b` |
| `build.py` | `80343ab7ed0129e6d0d6148ee74e688b3f1f273bbe08d9a0b6f8b587b57a5619` |
| `generated/coverage.csv` | `5a21a84ec3ee6bd3388ec5bb005c077e93b28f2336365ba9a9b1c52845d68078` |

## Eigene Abgabe und Aussagegrenze

Neu geschrieben wurden ausschließlich:

- `reviews/skeptic/a7/review.md`
- `reviews/skeptic/a7/findings.csv`
- `reviews/skeptic/a7/validation.md`

Die CSV verwendet UTF-8, Semikolon und exakt den Header `id;severity;references;finding;required_action`. Ihre neun IDs sind eindeutig. Branch-, Residue- und Modulreferenzen wurden gegen den unveränderten Quellenbestand geprüft. Die Schwereverteilung ist eine hohe, sieben mittlere und eine niedrige Auflage.

Start, Fortschritt, Bewertungsentscheidungen und Abschluss werden im zugewiesenen Run festgehalten. Keine anderen Dateien, Implementierung, Konfiguration, Worktrees, Agenten oder externe Systeme wurden durch diese Arbeit geändert. Kein direkter Taskstorezugriff erfolgte.

**Nicht getestet:** wirkliche Ressourcenisolation, tatsächliches Fencing, legitime menschliche Nachfolge, reale Alarmzustellung, Löschrecht, physische Sicherheit oder empirische Attraktion. Die inhaltlichen Findings sind analytische Gegenbeispiele und Entwurfsauflagen. Bestandene Strukturtests widerlegen sie nicht und begründen keine Betriebsfreigabe.
