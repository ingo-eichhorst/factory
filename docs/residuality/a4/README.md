# A4 — 50 Suchstrategien, extreme Verzögerungen und externe Ausfallerkennung

**Erweiterung vom 2026-09-09. Architektur- und Dokumentationsarbeit, keine eingerichtete Überwachung und keine ausgeführten Produkt-Stresstests.**

## Zuerst lesen

- **[Einfach erklärt](../einfach-erklaert.md)** — aktualisiert auf 17 Residues; Abschnitt 17 erklärt die externe Ausfallmeldung.
- **[Architektur A4](architecture-A4.md)** — konkrete Herleitung und Regeln, einschließlich des vollständigen Monitoringvertrags.
- **[50 Suchstrategien](generated/strategies.md)** — Qualitätsziel → Suchbewegung → drei konkrete Gegenproben.
- **[150 neue Stressoren](generated/stressors.md)** — S201–S350 mit Annahme, verbleibender Fähigkeit, Residue und Regel.

## Ergebnis dieses Durchgangs

Die bisherige Architektur dachte an Diagnose und Wiederaufbau nach Geräteverlust, aber **nicht ausdrücklich daran, den Betreiber ohne den ausgefallenen Rechner zu erreichen**. Das wird mit **R17 — unabhängige Ausfallerkennung** ergänzt.

Weitere neue Einsichten:

1. **Der Monitor braucht selbst einen überprüften Alarmweg.** Gleiches Stromnetz, gleicher Account oder gemeinsamer Updater können scheinbare Unabhängigkeit aufheben.
2. **Frisch empfangen ist nicht frisch beobachtet.** Gepufferte alte Heartbeats dürfen einen Ausfall nicht verdecken.
3. **Ein Timeout ist keine Rücknahme.** Die Außenwirkung kann nach Frist, Widerruf und sogar nach Ende der eigenen Erinnerung eintreten.
4. **Ein guter Tailwert kann eine schlechte Messung sein.** Wer nur fertige Arbeit zählt, sieht den langen Rest gar nicht. Wer Retry oder spekulative Kopien als kostenlose Beschleunigung behandelt, erzeugt neue Last und mögliche Doppelwirkungen.
5. **Wartbarkeit ist auch ein Ausfallthema.** Versionen, Kaltstart, temporärer Recoveryplatz, weggefallene Maintainer und gemeinsame Lieferketten müssen reale Gegenproben bekommen.
6. **Qualität ist nicht nur Geschwindigkeit.** Falsche Einheit, falscher Empfänger, fehlender Pflichtteil oder unverständlicher Alarm bleiben Fehler — auch bei grüner technischer Prüfung.

## Breite und Randbereiche

| Suchraum | Strategien | Beispiele |
|---|---|---|
| Beobachtbarkeit und unabhängige Kontrolle | Q01–Q06 | Rechner tot, Monitor tot, alte Pulse, Messapparat überlastet |
| Extreme Latenz und Nachfrage | Q07–Q18 | Jahre in der Queue, nie fertige Teilpopulation, zehntausend Unteraufrufe, Retrylawine |
| Energie und Kontrollzugang | Q19–Q20 | thermische Drosselung, unbestätigter Stop bei voller SSD |
| Security und Datenschutz | Q21–Q26 | falsches Scope-Credential, Promptinjection, signiertes bösartiges Update, Metadatenleck |
| Integrität, Recovery, Wartbarkeit und Testbarkeit | Q27–Q33 | gleich falsch kopierte Backups, alte Welt nach Restore, kaputte Versionskombinationen |
| Fachqualität, Menschen und Organisation | Q34–Q37 | falsche Einheiten, unlesbare Alarme, jahrelange Entscheidung, widersprechende Vertreter |
| Anbieter, Kosten, Recht und Safety | Q38–Q41 | gemeinsame Accountsperre, späte Kosten, Untersagung während Dispatch, physische irreversible Wirkung |
| Netz, Zeit, Erkenntnis und Anreize | Q42–Q45 | 30 Tage Satellitenverzögerung, Uhrsprung, gemeinsame falsche Quelle, manipulierte Kennzahl |
| Zielkonflikte, ferne Zukunft und Unmöglichkeit | Q46–Q50 | Energiesparen gegen Frist, Restore 2080, alle Anker verloren, adversarielle Reihenfolge |

**50 Strategien insgesamt in dieser Suchrunde, jeweils drei Karten: 150 Ergänzungen.** Die alten 20 Perspektiven sind keine zusätzlichen Strategien dieser Liste. Es wurden weder Häufigkeiten noch Latenzperzentile gemessen. Extremjahre und Größenordnungen sind bewusste Gedankenexperimente, keine Prognosen. Quellenumfang und methodische Grenzen stehen in [sources.md](sources.md).

## Verdichtung und Architekturänderung

- **350 Karten insgesamt:** 200 historische + 150 neue.
- **17 Residues:** die bisherigen 16 plus R17. Keine 50 neuen Dienste.
- **740 direkte Inzidenzen** in der gemeinsamen **350 × 17**-Matrix.
- **12 neue Regelpräzisierungen C37–C48**, zusammen mit den bisherigen Regeln 48 Obligationen.
- **A4 ergänzt A3**, statt seine historischen Zahlen rückwirkend zu verändern.
- **39 Karten** benötigen weiterhin mindestens eine der offenen C33–C36: harte feindliche Isolation, unabhängige verlorene Wirkungsevidenz, tragfähiger Fachmaßstab oder konkrete Rechtsentscheidung.
- Acht neue Incident-Kaskaden I13–I20 und zwölf neue vorgeschlagene Experimente T18–T29 ergänzen die bisherigen zwölf Kaskaden und 17 Experimente.

Die Verdichtung hat auch **Nicht-Änderungen** ergeben: kein automatischer Ersatzwriter nach Monitoralarm, kein neuer Timeout als Todesbeweis, keine pauschale Doppelzustellung zur Latenzsenkung, kein Zertifikat aus Matrixfüllung und kein universeller Schutz bei Verlust aller Vertrauensanker.

## Daten und Nachverfolgung

Kanonische neue CSVs verwenden **Semikolon**:

- [strategies.csv](strategies.csv): genau 50 Suchstrategien mit Qualitätsziel und Quellenbasis.
- [stressors.csv](stressors.csv): genau drei Karten pro Strategie.
- [controls.csv](controls.csv): neue Regeln C37–C48; bestehende Regeln werden aus der Baseline gelesen.
- [residues.csv](residues.csv): neues R17; zusätzliche Verstärkungen alter Residues stehen in der Control-Zuordnung und verändern deren historische Definition nicht rückwirkend.
- [residue-dependencies.csv](residue-dependencies.csv): sechs zusätzliche bedingte Voraussetzungen.
- [incidents.csv](incidents.csv): acht geordnete Kaskaden, keine simulierten Incidentergebnisse.
- [experiments.csv](experiments.csv): zwölf neue **vorgeschlagene**, nicht ausgeführte Experimente.

Abgeleitete CSVs verwenden **Komma**:

- [Gemeinsame Inzidenzmatrix](generated/incidence-matrix.csv)
- [Vollständige Herleitung: Karte → Residue → Regel → Revision → Experiment](generated/traceability.csv)
- [1750 Revisionsbewertungen auf demselben 350-Karten-Korpus](generated/architecture-assessments.csv)
- [Gemeinsame Auswertung](generated/summary.md)

`benannt` bedeutet weiterhin nur: Alle zugeordneten Entwurfsregeln sind vorhanden. Bei S345 sind die Grenzen vollständig benannt und trotzdem kann bei Verlust sämtlicher Beobachter und Empfänger **kein Alarm** erfolgen. Additive Regeln verbessern diese syntaktische Auswertung konstruktionsbedingt. Daraus folgt keine gemessene Resilienzquote.

## Prozess und tatsächlich ausgeführte Prüfungen

1. Monitoringlücke gegen A3 geprüft und die historische Baseline erhalten.
2. Öffentliche Qualitäts-, SRE-, Security- und Alertingquellen recherchiert; Leseumfang und Grenzen dokumentiert.
3. 50 explizite Suchbewegungen und 150 Gegenproben kuratiert.
4. Wiederkehrende Fähigkeiten verdichtet, R17 ergänzt und zwölf konkrete Regeln formuliert.
5. Besonders lange Verzögerungen, gemeinsame Ausfälle und Kaskaden gegen die Regeln gestellt. Unmöglichkeitsfälle bleiben offen oder begrenzen die Nutzung.
6. Gemeinsame Matrix abgeleitet, einfache Erklärung aktualisiert und Modellkonsistenz geprüft.

```sh
# Vom Factory-Projektroot, nur Python-Standardbibliothek:
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a4/test_model.py
python3 docs/residuality/a4/build.py --check

# Historische Baseline zusätzlich prüfen:
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/test_matrices.py
python3 docs/residuality/build_matrices.py --check

# Nur nach bewusster Datenänderung die sieben A4-Ableitungen regenerieren:
python3 docs/residuality/a4/build.py
```

**17 A4-Dokumentmodelltests und die 11 Baseline-Dokumentmodelltests erfolgreich.** Geprüft werden insbesondere Zuordnungen, die zwölf leicht lesbaren Architekturherleitungen gegen die kanonischen Daten, unveränderte historische Inzidenzen, Zeitstand der neuen Regeln, offene Verpflichtungen, negative Tabellenfälle und deterministische Ableitung. Diese Tests prüfen weder echte Alarmzustellung noch Factory unter Last. Die 29 Produkt-/Betriebsexperimente sind weiterhin Vorschläge.

## Nächster sinnvoller Schritt

Zuerst **T18–T20** zusammen mit Zuständigkeit/Alarmweg aus T27 und unabhängigen Ausfalldomänen aus T29 konkretisieren. Danach einen separat genehmigten Ende-zu-Ende-Test: Rechner nicht erreichbar → unabhängige Erkennung → Nachricht beim tatsächlichen Empfänger. Anbieter, Empfänger, übertragene Daten, Kosten, Zeitgrenzen und Wartungsregeln sind vorher ausdrücklich festzulegen.

Task-Run: `b419b5f6-0e56-43ac-9de9-3446a225a4d1`.
