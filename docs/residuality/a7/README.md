# A7 — alle 350 Stressoren, ihre Residues und die modulare Zielarchitektur

**Nachträge nach Abschluss des 350er-Reviews:** [S351: Unterstützung eines weiteren Betriebssystems](addenda/S351-weiteres-betriebssystem.md) mit sieben bedingten Zweigen und begründeter Wiederverwendung bestehender Kandidaten. Damit sind 351 Stressoren erfasst, aber der nachfolgende eingefrorene Reviewstand und seine Kennzahlen bleiben beim 350er-Korpus. S351 ist noch nicht unabhängig gegengeprüft. Zwei zusätzliche Dokumenttests prüfen seine sieben Referenzen und die Trennung vom alten Korpus; sie laufen im unten genannten `verify.py` mit.

**Erste lokale Vertragsprüfung:** [P02-Teilergebnis](validation/p02/README.md): vier Rust-Integrationstests bestanden; alte Snapshot-Nachgeschichte und heutige Autorität bleiben begrenzt beziehungsweise ungeprüft. Keine vollständige P02-Freigabe.

## Die drei wichtigsten Dokumente

1. **[Vollständige Herleitung für alle 350 Stressoren](generated/stressoren.md)** — pro Stressor die möglichen Zustände, Bedingungen, zugeordneten Residues, Gründe, Prüfbedarf und Architekturfolge.
2. **[Residue-Katalog](generated/residues.md)** — pro Kandidat der erhaltene Gegenstand, die nutzbare Fähigkeit, ihre Voraussetzungen, Ausfallgrenzen, zuständigen Module und sämtliche zugeordneten Szenariozweige.
3. **[Modulare Zielarchitektur A7](zielarchitektur.md)** — wie die Fähigkeiten gemeinsam umgesetzt werden sollen, ohne einen Dienst pro Residue zu bauen.

Alle drei sind auf Deutsch. Die vollständige Herleitung ist absichtlich lang: Sie ersetzt nicht 350 konkrete Fälle durch wenige ausgewählte Beispiele.

## Was jetzt vollständig vorliegt

Vier Analysten haben ihre Bereiche erneut und getrennt bearbeitet. Nicht nur die früheren 19 ausdrücklich benannten Attraktorhypothesen, sondern **alle 350 Stressoren** besitzen nun konkrete fallbezogene Zustands- und Residue-Dispositionen.

**Unveränderte unabhängige Abgaben:**

| Bereich | Stressoren | Bedingte Zweige | Herkunftserhaltende Residue-Kandidaten |
|---|---:|---:|---:|
| Betrieb | 100 | 273 | 33 |
| Organisation und Veränderung | 100 | 214 | 38 |
| Beobachtung und Zeit | 75 | 171 | 42 |
| Bedeutung und Grenzen | 75 | 194 | 39 |
| **Gesamt** | **350** | **852** | **152** |

Nach dem Gegenreview kommen **drei ausdrücklich markierte Zweige und KOR001** hinzu. Der aktuelle korrigierte Stand ist damit **350 Stressoren, 855 Zweige und 153 Residue-Kandidaten**. Die ursprünglichen Abgaben wurden dafür nicht umgeschrieben; [hashgebundene Nachträge](review-adjustments.json) halten die Änderungen fest.

Das ist keine bewiesene kleinste Menge weltweit verschiedener Residues. Einige Kandidaten überlappen; ihre Bedingungen bleiben sichtbar. Die Anzahl wurde nicht vorgegeben. Dieselben Kandidaten werden bei passenden mehreren Stressoren wiederverwendet, etwa ein Subscriber-Cursor aus dem Beobachtungsbereich nun auch für S054.

**855 Zweige sind nicht 855 Attraktoren.** Sie unterscheiden Rückkopplungshypothesen, vorübergehende Störungen, äußeren Zwang, Halt, Ungewissheit, Abschluss, Verlust, Eskalation, bloßen Schreibkonflikt und offene Dynamik. Die ursprünglichen 24 ungeklärten Szenarien erhalten ausdrücklich bedingte Alternativen und benannte fehlende Voraussetzungen — keine erfundene empirische Lösung.

## Wie „allen zugewiesen“ zu verstehen ist

Jeder Zweig hat eine ausdrückliche Residue-Disposition:

- **Bedingt:** eine konkrete Restfähigkeit wäre unter den genannten Voraussetzungen nutzbar.
- **Teilweise:** ein engerer Gegenstand bleibt nutzbar; die weitergehende Fähigkeit fehlt.
- **Keines:** für den ausdrücklich betrachteten Gegenstand gibt es unter der gesetzten Annahme kein brauchbares Residue.
- **Ungeklärt:** die benötigte Information oder Voraussetzung reicht für eine positive Erhaltungsbehauptung nicht aus.

Die letzten beiden Fälle werden **nicht ausgelassen**. Beispiel: Wenn alle Geräte, Sicherungen, Schlüssel und Erinnerungen verschwinden, wird keine zusätzliche Rettungskopie erfunden. Bei einer ununterscheidbar falschen Welt wird kein perfekter Wahrheitsprüfer hinzugefügt. Das ist eine konkrete negative Zuordnung, keine gewonnene Schutzfähigkeit.

Manche Zweige betrachten einen ausdrücklich anders vorbereiteten Entwurf. Eine vorab unabhängige Kopie kann einen begrenzten Hardwareverlust abfangen; sie überlebt nicht die anders lautende Prämisse „jede Kopie vernichtet“. Deshalb gehören Bedingungen und Grenzen immer zur Zuordnung. Die Zweige sind weder eine vollständige Menge aller denkbaren Abläufe noch sämtlich gegenseitig ausschließende Alternativen.

## Was die Zielarchitektur daraus macht

**16 logische Modulverantwortungen** bündeln die Kandidaten. Das sind keine 16 zusätzlichen Server und keine neue feste Residue-Anzahl.

Die zentralen Änderungen sind:

- ein gemeinsamer Faktenkern statt paralleler versteckter Wahrheiten;
- getrennte Verträge für Identität, aktuelle Befugnis, lokalen Versuch, echte Teilnehmerannahme und wirksamen Schreiberausschluss;
- gemeinsame Arbeits-, Versuchs-, Zeit- und Kostenbegrenzung statt unkoordinierter Wiederholungen;
- konkrete Inhalte und fachliche Abnahme statt „Dateipfad genannt = Arbeit erfolgreich“;
- Wiederherstellung als tatsächlich nutzbarer Satz aus Inhalt, Leser, Schlüsselzugang, Werkzeugen und Rechten;
- unabhängige Beobachtung ohne automatische Reparatur- oder Geschäftsautorität;
- getrennte Meldung, menschliche Quittung, offene Fallakte und überprüfte Reparatur;
- ausdrückliche Daten-, Rechts-, Angreifer- und Betriebsgrenzen statt unbelegter Universalgarantien.

[Modulübersicht mit allen Kandidaten](generated/modules.md) · [Einzelne Verantwortungszuordnung](module-bindings.csv) · [Abhängigkeiten](module-dependencies.csv)

Gemeinsame Modulzugehörigkeit ist keine automatische Verschmelzung von Residues. Ein Modul kann dieselbe Speicherschnittstelle benutzen und trotzdem mehrere unabhängig prüfbare Fähigkeiten erhalten. Jeder Kandidat hat einen primären Vertragseigner; andere liefern benannte Mitwirkung, ohne dadurch automatisch zur globalen Startvoraussetzung zu werden.

## Review und Nachweise

Die vier Originalabgaben sind [vor Gegenreview eingefroren](discovery-freeze.json). Die [Review-Dispositionen](review-dispositions.md) dokumentieren neun konkrete Korrekturen. Der unabhängige Nachreview bestätigt sie als auf Dokumentebene behoben; verbleibende Betriebs- und Nachweisgrenzen bleiben ausdrücklich offen. Die ursprünglichen A6-Dokumente und Tabellen bleiben erhalten.

**Kein tatsächlicher Factory-Attraktor und keine vollständige Betriebsresilienz wurde hier nachgewiesen.** Positive Dokumentprüfungen bedeuten nachvollziehbare Zuordnung, nicht funktionierenden Schutz. Der [Prüfplan](pruefplan.md) nennt kombinierte Gegenversuche; reale Teilnehmer, Menschen, Alarmwege und physische Anlagen brauchen gesonderte Erlaubnis.

## Dateien und Reproduktion

**Abschlussprüfung:** 29 A7-Dokumenttests bestanden, einschließlich negativer Prüfungen für geänderte eingefrorene Dateien, umgeordnete Zweigreferenzen und einen eingefügten Startzyklus. Der vollständige Gegenreview und der gezielte Nachreview sind abgeschlossen. Keine Produkt- oder Livefehlerprüfung wurde dadurch ersetzt.

- [Prozess](process.md) und [Agenten-/Aufgabenmanifest](review-manifest.json).
- [Review und neun Korrekturauflagen](review-dispositions.md), [getrennte Teilverträge](contract-facets.csv) und [operationenspezifische Betriebsmodi](operation-boundaries.csv).
- Unabhängige Berichte: [Betrieb](../../../reviews/operations/a7/report.md), [Organisation](../../../reviews/governance/a7/report.md), [Beobachtung](../../../reviews/observability/a7/report.md), [Grenzen](../../../reviews/boundaries/a7/report.md).
- Maschinenlesbar: [350er-Übersicht](generated/coverage.csv), [alle Zweige](generated/branches.csv), [Zweig → Residue → Modul](generated/traceability.csv), [Quellhashes](generated/source-hashes.json), [Zählwerte](generated/summary.md).
- Quelldaten der Analysten sind Semikolon-CSV und JSON. `modules.csv` und `module-dependencies.csv` verwenden Semikola; die ausdrücklich erzeugte `module-bindings.csv` und die generierten CSVs verwenden Kommas. Alle Dateien sind UTF-8.

```sh
# Vollständiger Nurlese-Prüfaufruf, keine Produktionsversuche:
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a7/verify.py

# Einzelne Schritte:
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a7/build.py --inventory-only
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a7/bind_modules.py --check
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a7/build.py --check
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a7/test_model.py
```

`build.py` erzwingt den Quellenfreeze, wendet die expliziten Nachträge an, prüft die deklarierten Startabhängigkeiten und erzeugt die Dateien unter `generated/` ausschließlich unter seinem Eigentumsmarker. Es klassifiziert keine Szenarien selbst und beweist keine Kausalität. `bind_modules.py` enthält ausdrücklich gesetzte Zuordnungen des Koordinators, keine automatische Ähnlichkeitssuche. Inhaltliche Änderungen brauchen erneute Prüfung; eine bloße Neugenerierung genügt nicht.

A7 ist der aktuelle **Entwurf** innerhalb dieses Analysepakets. Es wurden keine Produktfunktionen implementiert, angenommenen ADRs geändert, externen Dienste eingerichtet oder bestehenden Arbeitsprodukte überschrieben. Die neuen Vorschläge sind über den bestehenden ADR-/Backlogprozess zu entscheiden.
