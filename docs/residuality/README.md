# Factory Residuality Lab — A7 und additive Nachträge

Architekturanalyse in Anlehnung an Barry O’Reilly. Baseline vom 2026-09-08: 200 Stressoren/A0–A3; Erweiterung vom 2026-09-09: 150 zusätzliche Stressoren aus 50 Suchstrategien/A4. **Entwurf und Prozessartefakte, keine behaupteten bestandenen Resilienztests.** Baseline-Run: `ba509078-d4c6-4dc2-8f1b-170329688d76`; A4-Run: `b419b5f6-0e56-43ac-9de9-3446a225a4d1`.

## Einstieg

**Neu:** [S351 — ein weiteres Betriebssystem unterstützen](a7/addenda/S351-weiteres-betriebssystem.md), als separater Nachtrag mit sieben Zweigen. 351 Stressoren sind damit erfasst; der unabhängig geprüfte 350er-Korpus bleibt unverändert. [P02 ist inzwischen lokal teilweise geprüft](a7/validation/p02/README.md): vier Rust-Tests, keine vollständige Betriebs-, Teilnehmer- oder Widerrufsvalidierung.

**Aktuell: [A7 — alle 350 Stressoren und modulare Zielarchitektur](a7/README.md).** Vollständige fallbezogene Zustands-/Residue-Dispositionen, konkrete verbleibende Fähigkeiten und begründete Modulverträge ohne vorgegebene Residue-Anzahl. Direkt zur [vollständigen Herleitung](a7/generated/stressoren.md), zum [Residue-Katalog](a7/generated/residues.md) oder zur [Zielarchitektur](a7/zielarchitektur.md). Bedingungen, negative Grenzen und der Gegenreview bleiben Teil jeder Aussage.

**Frühere Teilmenge, einfach auf Deutsch: [Attraktor → Stressoren → Residue → Architekturänderungen](a6/attraktoren-residues-architektur-einfach.md).** Ein separates Dokument mit je einem Abschnitt für alle 19 ausdrücklich benannten A6-Attraktorhypothesen, ihren vollständigen Stressorzuordnungen, bedingten Restfähigkeiten und konkreten Entwurfsvorschlägen.

**Unabhängige Grundlage: [A6 — what the independent agents changed](a6/README.md).** Vier getrennte Analysten haben alle 350 Szenarien mit eigenen Verläufen und Begründungen versehen; ein fünfter hat Methode und Ergebnisse kritisch geprüft. A6 erhält Unterschiede, ungeklärte Fälle und Gegenbeispiele, korrigiert frühere Modellinterpretationen und behauptet weiterhin keine bewiesene Residue-Anzahl oder real beobachtete Factory-Attraktoren.

**Vorherige Zustandsanalyse: [A5 — states first, residues afterwards](a5/README.md).** Sechs ausführbare Spielmodelle, mit Mechanismen aus 55 Stressoren verknüpft. A6 schränkt unter anderem die Alarm-/Incident-Interpretation und die Aussagekraft der Kandidatenverknüpfung ausdrücklich ein. Historische Modelle und Tabellen bleiben reproduzierbar.

**Historische einfache Erklärung: [Vom Stressor zur Architekturentscheidung](einfach-erklaert.md).** Erklärt die bisherigen 17 A3/A4-Kategorien, ohne Matrixkenntnisse vorauszusetzen.

**Vorherige Erweiterung: [A4 — 50 Suchstrategien, extreme Latenzen und unabhängige Ausfallerkennung](a4/README.md).** Enthält die [Architekturänderungen](a4/architecture-A4.md), [50 Strategien](a4/generated/strategies.md), [150 neuen Karten](a4/generated/stressors.md) und die [gemeinsame 350×17-Matrix](a4/generated/incidence-matrix.csv).

Die folgenden Links und Zahlen dokumentieren weiterhin die **unveränderte 200-Karten-Baseline A0–A3**:

1. **[Abschlussbericht](final-report.md)** — Ergebnisse, offene Grenzen und vorgeschlagene nächsten Gates.
2. **[Revidierte Architektur A3](architecture-A3.md)** — konsolidierter Entwurf und konkrete Änderungen gegenüber bestehenden Verträgen.
3. [Stressor-Katalog](generated/stressors.md) — lesbare Sicht auf alle 200 Karten aus 20 Perspektiven.
4. [Verdichtung und Trade-offs](synthesis.md) — warum 16 Residues und nicht 200 Einzelmaßnahmen.
5. [Matrixauswertung](generated/summary.md) — rein modellbezogene Lücken je Architekturstand.

## Prozessdokumente

- [Methodik und Evidenzgrenzen](methodology.md)
- [Quellen und statische Codebefunde](sources.md)
- [A0 — bestehender Architekturvertrag](iterations/A0-baseline.md)
- [A1 — lokale Sicherheitsübergänge](iterations/A1-local-safety.md)
- [A2 — Recovery und aktuelle Autorität](iterations/A2-recovery-authority.md)
- [A3 — Nachfolge, Daten- und Einsatzgrenzen](iterations/A3-boundaries.md)
- [Vorherige erste Analyse](../residuality-stressor-analysis.md), unverändert erhalten

## Kanonische Tabellen der Baseline

CSV mit **Semikolon** und UTF-8; auch direkt in einer Tabellenkalkulation verwendbar.

| Datei | Inhalt |
|---|---|
| [perspectives.csv](perspectives.csv) | 20 Perspektiven mit Leitfragen und Quellenbasis |
| [stressors.csv](stressors.csv) | 200 Situationen, Annahmen, Restfähigkeiten, Obligationen und Grenzen |
| [residues.csv](residues.csv) | 16 Residues mit Mindestdienst, Verstärkung, Kosten und Heimat |
| [controls.csv](controls.csv) | 36 Entwurfsobligationen, vier bleiben offen |
| [residue-dependencies.csv](residue-dependencies.csv) | 26 bedingte Voraussetzungen; nicht mit direkter Inzidenz verwechseln |
| [incidents.csv](incidents.csv) | Zwölf geordnete Stressor-Kaskaden mit nichtlinearen Wechselwirkungen |
| [experiments.csv](experiments.csv) | 17 vorgeschlagene Produkt-/Betriebsexperimente, keines hier ausgeführt |

## Abgeleitete Tabellen der Baseline

CSV mit **Komma** und UTF-8. Nicht von Hand editieren; Eigentumsmarker in `generated/OWNERSHIP.txt` erlaubt ausschließlich diesem Dokumentgenerator die Regeneration.

| Datei | Inhalt |
|---|---|
| [incidence-matrix.csv](generated/incidence-matrix.csv) | 200 × 16 binäre Matrix, 443 direkte Inzidenzen |
| [architecture-assessments.csv](generated/architecture-assessments.csv) | 800 Stressor-/Revisionsbewertungen, jeweils benannte und fehlende Obligationen plus verbleibende Grenze |
| [residue-overlap.csv](generated/residue-overlap.csv) | Gemeinsame Inzidenzen; keine statistischen Wahrscheinlichkeiten |
| [traceability.csv](generated/traceability.csv) | Stressor → Residue → Obligation → Architekturrevision → vorgeschlagenes Experiment |

`1` heißt zugeordnet, nicht bestanden. `benannt` heißt Entwurfsantwort vorhanden, nicht implementiert oder beherrscht. So kann S190 (Totalverlust aller Information) vollständig benannte Grenzen besitzen, ohne irgendeine wiederherstellbare Arbeit zu hinterlassen.

## Reproduzieren und prüfen

Die folgenden Befehle prüfen die historische Baseline. Für A4 siehe die [separaten Prüfkommandos](a4/README.md).

Nur Python-Standardbibliothek. Diese Befehle verändern keine Factory-Runtime, sprechen keine Plugins an und brauchen keine Produkt-Testinstallation.

```sh
# Vom Projektroot:
python3 docs/residuality/build_matrices.py --check
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/test_matrices.py

# Nur nach bewusster Änderung der Quelltabellen regenerieren:
python3 docs/residuality/build_matrices.py
```

Ausgeführt: **11 Dokumentmodelltests erfolgreich**, darunter fehlende/unbekannte Controls, absichtlich entzogene Entwurfsantwort, offene Obligationen und Totalverlust trotz vollständiger Benennung. `--check` bestätigt die sieben abgeleiteten Dateien. Die Zwischenstände 160/A1 und 180/A2 werden in den Tests aus dem finalen Korpus reproduziert; der finale Bericht verwendet stets dieselben 200 Karten für alle vier Revisionen.

Kein `check.sh`, Cargo-Produkt-Test, Crashdrill, Restore der Live-DB, externer Versand, Agentstart oder Worktree-Eingriff wurde für diesen Auftrag ausgeführt. Die 11 Tests prüfen die **Analysewerkzeuge**, nicht Factory. Empfehlungen sind keine automatische Umsetzungserlaubnis.
