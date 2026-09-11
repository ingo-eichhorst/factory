# Abschlussbericht — Factory unter 200 Stressoren

## Ergebnis in einem Satz

**Factory sollte nicht möglichst viel selbst reparieren, sondern unter schwindender Evidenz seine Handlungsbefugnis kontrolliert verkleinern — und dabei Arbeit, Entscheidungen und einen sicheren Weg zur Fortsetzung erhalten.**

Die breite Analyse verstärkt diesen Ansatz, korrigiert ihn aber auch: Sicherer Stillstand ist kein vollständiges Betriebsmodell. Menschen, Budgets, Inhalte, Schlüssel, Rechtsgrundlagen und Qualitätsmaßstäbe können genauso ausfallen wie Plugins. Sie müssen im Architekturvertrag vorkommen, ohne dadurch alle zu neuen Kernkomponenten zu werden.

## 1. Gelieferter Umfang

- **200 Stressoren aus 20 Perspektiven**, jeweils mit gebrochener Annahme, verbleibender Fähigkeit und Grenze.
- **16 Residues**, aus wiederkehrenden Restfähigkeiten verdichtet, mit Mindestdienst, Verstärkung, Preis und architektonischer Heimat.
- **200 × 16 Inzidenzmatrix**, 443 explizite direkte Zuordnungen; zusätzlich Überlappungsmatrix, 26 bedingte Residue-Abhängigkeiten und zwölf geordnete Incident-Kaskaden.
- **Vier Architekturstände A0–A3**, davon drei Überarbeitungen aus Matrix- und Gegenprobenbefunden.
- **36 überprüfbare Entwurfsobligationen**: 32 in A0–A3 benannt, vier ausdrücklich offen.
- **17 vorgeschlagene Experimente** mit Falsifikationskriterien; 11 tatsächlich ausgeführte Tests prüfen ausschließlich Tabellen und Ableitungslogik.

Der konsolidierte Architekturentwurf steht in [architecture-A3.md](architecture-A3.md). Prozess und Daten sind über [README.md](README.md) zugänglich. Bestehende ADRs, Implementierung und Livebetrieb wurden nicht geändert.

## 2. Die wichtigsten Erkenntnisse

### A. Ungewissheit wird an der Runtime-Grenze noch zu leicht in Gewissheit verwandelt

Im aktuellen Recovery-Arbeitsstand kann `Unavailable` bzw. nicht akzeptierte Beobachtung zur Leasefreigabe führen; fehlende Session-ID wird als Match behandelt. Damit versagt gerade der beabsichtigte Restbetrieb „unbekannte Arbeit bleibt geschützt“.

**Entwurfsänderung:** Identität, Autorität, Aktualität und Lebendigkeit getrennt prüfen; Endevidenz vor Leasefreigabe. Ein Timeout ist kein Todesbeweis. Dies hat Vorrang vor zusätzlicher Runtime-Automatisierung.

### B. Restore ist semantisch gefährlicher als gewöhnlicher Restart

Ein alter Snapshot kennt spätere Zustellungen, Wirkungen und Widerrufe nicht. Auch ein darin wartender Task ohne Versuch kann bereits ausgeführt sein. Ein lokaler Lock schützt außerdem nicht vor einem zweiten Klon auf anderer Hardware.

**Entwurfsänderung:** Recovery-Inkarnation mit zunächst gesperrtem Dispatch, aktuelle Effekt-/Freigabeevidenz und tatsächlich wirksames Teilnehmerfencing oder expliziter Single-active-Betrieb. Keine automatische Fortsetzung aus Snapshot-Leere.

### C. Die dichteste Kopplung liegt zwischen Wirkung und Befugnis

R02/R09 teilen 16 Karten. Ein Versuch, eine fachliche Operation und eine Freigabe sind verschiedene Fakten. Eine global freigegebene Task-ID oder ein Zustellcounter allein ist zu grob.

**Entwurfsänderung:** Actor-, Payload-, Empfänger-, Versions- und Gültigkeitsbindung; neue Prüfung nahe am Dispatch. Resume legitimiert nicht automatisch eine erneute Geschäftsaktion. Widerruf nach Annahme ist kein Rückgängigmachen.

### D. Optionalität braucht einen geschützten Kontrollpfad und globale Budgets

Scope-lokale Subprozesse können zusammen dieselbe Maschine überlasten. Blockierte Worker können alle Slots belegen; ein Bediener kann in Freigaben ertrinken. Safe Mode selbst benötigt einen startbaren Kern und verfügbare Ressourcen.

**Entwurfsänderung:** Begrenzte Pluginaufrufe plus globale Admission/Fairness, Kontrollreserve, minimaler lokaler Bootstrap und begrenzte menschliche Entscheidungsqueue. Kein zweiter privilegierter Writer und keine automatische Zustimmung nach Ablauf.

### E. Dauerhaft gespeicherte Bytes sind nicht automatisch wiederherstellbare Bedeutung

Recovery benötigt Inhalte, Idempotenzfakten, Formate, Schlüssel und Verständnis. Ein Hash ersetzt keine gelöschte Datei. Rechtskonflikte können vollständigen Replay unzulässig machen. Ein verschwundener Anbieter oder Maintainer kann intakte Bytes unbrauchbar hinterlassen.

**Entwurfsänderung:** Klassifiziertes Recovery-Set, unabhängige Ausfalldomäne, separater Schlüsselprozess, ehrliche Import-/Replaygrenze und lesbarer Ausstieg. Kein unbedingtes „alles ist regenerierbar“.

### F. Die extremen Fälle verlangen ehrliche Nicht-Zusagen

S181–S190 enthalten unter anderem Sonnensturm, Wiedererwachen 2046, gemeinsame Irrtümer aller Prüfer, unvereinbare Rechtsforderungen und vollständigen Informationsverlust. Daraus folgt nicht, Factory für jedes denkbare Universum auszubauen.

**Entwurfsänderung:** Trusted Single User bleibt die Einsatzgrenze. Totalverlust bleibt Verlust; ein widersprüchliches Mandat bleibt widersprüchlich; Übereinstimmung bleibt fehlbar. Architektur muss solche Grenzen lesbar machen, nicht wegdefinieren.

## 3. Was die Iterationen tatsächlich verbessert haben

Bewertung aller Stände auf **demselben finalen 200-Karten-Korpus**:

| Stand | Alle zugeordneten Obligationen benannt | Teilweise | Keine |
|---|---:|---:|---:|
| A0: bestehender Minimalvertrag | 9 | 89 | 102 |
| A1: lokale Sicherheitsübergänge und Budgets | 41 | 123 | 36 |
| A2: Recovery, Gegenwartsbefugnis und Kontrollbetrieb | 120 | 76 | 4 |
| A3: Nachfolge, Retention, Erkenntnis- und Einsatzgrenzen | 178 | 22 | 0 |

**Das sind keine Erfolgsquoten.** Die Berechnung prüft nur, ob explizit zugewiesene Entwurfsantworten vorliegen. Additive Revisionen verbessern diesen Wert konstruktionsbedingt. Die 178 vollständig benannten Fälle umfassen auch kontrollierten Stillstand, Einsatzgrenzen und akzeptierten Verlust. Die Tabelle rechtfertigt insbesondere nicht „89 % resilient“.

Dominante Restbetriebsarten im Korpus: **40 degradiert, 106 halten, 20 Einsatzgrenze, 20 offen, 14 Verlust**. Diese Zuordnung ist ebenfalls eine Hypothese, keine gemessene Ausfallverteilung. A3 braucht nach den letzten Gegenproben keine weiteren Residue-IDs, wohl aber sechs zusätzliche Vertragspräzisierungen; das ist kein bewiesener Sättigungspunkt.

## 4. Offene Fragen werden nicht versteckt

22 Karten benötigen weiterhin mindestens eine der vier offenen Obligationen:

| Offen | Entscheidung / fehlende Evidenz | Konsequenz |
|---|---|---|
| C33 | Harte Isolation bzw. anderer Trustanchor für feindliche Betriebsarten | Nicht als Same-User-v1 unterstützen; separates Mandat und Nachweis nötig. |
| C34 | Unabhängige Effekt-Evidenz, wenn Teilnehmerhistorie endgültig fehlt | Keine automatische Wiederholung. Ausgang kann dauerhaft unbekannt bleiben. |
| C35 | Domänenspezifischer tragfähiger unabhängiger Qualitätsmaßstab | Kein unbelegtes Qualitätsurteil oder vermeintliche allgemeine Kompetenzgarantie. |
| C36 | Konkrete rechtliche Lösch-/Aufbewahrungs-/Zuständigkeitsentscheidung | Menschliche legitimierte Entscheidung; mögliche begrenzte Replayfähigkeit offenlegen. |

Daneben bleiben im Entwurf benannte Maßnahmen **unimplementiert bzw. unbewiesen**: Eventmodell-Cutover, vollständiger Plugin-Host, neue Recovery-/Freigabeverträge und reale Restore-/Lastdrills. Die genaue Liefergrenze gehört in den bestehenden Backlog und in ratifizierte ADRs, nicht in einen zweiten dauerhaften Schattenplan.

## 5. Empfohlene Umsetzung in drei Gates

### Gate 1 — vor zusätzlicher automatischer Runtime-Recovery

T01–T04 als isolierte Fixture-/Fault-Injection-Tests: fehlende Identität, nicht beobachtbar aber lebendig, Crashfenster, Restore ohne spätere Historie und zwei Klone. Die statischen Recoverybefunde erneut gegen aktuellen Code bestätigen und dann gezielt korrigieren. Kein Produktionscrash für den ersten Nachweis nötig.

### Gate 2 — vor unbeaufsichtigtem Betrieb

Geltung von ADR 0003/0007 und A3 ratifizieren; kanonischen Task-Lifecycle, Referenzintegrität, Minimal-Bootstrap, Admission, Freigaben und genehmigten Cutover nachweisen (T05–T14/T17). Backup-Recovery auf isolierter Umgebung üben; Live-Drills und Off-device-Einrichtung bleiben freigabepflichtig.

### Gate 3 — vor erweitertem Produktversprechen

Für echte Domänen passende Verifikation und Limits vereinbaren (T15). Neue Mandanten-/Remote-/Safety-/HA-Nutzung bekommt eigenes Mandat und Isolation-/Kapazitätsnachweis (T16). Nicht durch großzügigere Plugin-Konfiguration oder mehr `AGENTS.md`-Text einschleusen.

## 6. Owner-Entscheidungen, die jetzt den größten Hebel haben

1. **Minimalbetrieb bei Abwesenheit:** Welche internen reversiblen Arbeiten dürfen weiterlaufen, welche müssen auf menschliche Entscheidung warten?
2. **Recovery-Ziele:** Welcher Datenverlust (RPO) und welche Wiederanlaufzeit (RTO) sind akzeptabel, und wer hält die genehmigte unabhängige Recovery-Menge samt Schlüsselvertretung vor?
3. **Globale Budgets:** Welche Grenzen gelten für Kosten, Queue, Speicher, aktive Prozesse und maximale menschliche Entscheidungsbelastung? Welche Scopes erhalten im Engpass Vorrang?
4. **Trust- und Autoritätsgrenze:** Bleibt v1 Single User mit vertrauenswürdigen Workloads, und wer darf wen legitim vertreten?
5. **Datenhaftung und Fachurteil:** Wer entscheidet Retentionkonflikte und ratifiziert tatsächliche fachliche Akzeptanzkriterien?

## 7. Qualität und Grenzen dieses Ergebnisses

Geprüft wurden Tabellenstruktur, IDs, direkte Zuordnungen, Symmetrie der Überlappung, Versionslogik, Fortbestand offener Obligationen, negative Modellfälle und reproduzierbare Zwischenstände. Die abgeleiteten Dateien stimmen mit den Quelltabellen überein. Es wurden **keine T01–T17-Experimente und keine Factory-Produkt-/Live-Tests** ausgeführt.

Die 200 Stressoren sind kuratiert, nicht zufällig; der gleiche Analyst erstellte Karten, Zuordnungen und Gegenproben. Es gab keine unabhängigen Interviews oder externe Primärliteraturprüfung. Die Zahl 200 erfüllt den vereinbarten Umfang, nicht eine Vollständigkeits- oder Konvergenzgarantie. Ein unabhängiger Review sollte insbesondere Spalten mit wenigen direkten Karten, scheinbar gut abgedeckte Kaskaden und die Sinnhaftigkeit der Qualitäts-/Rechtsgrenzen angreifen.

**Fazit:** A3 ist ein konkreter revidierter Architekturentwurf mit nachverfolgbaren Gründen — kein Resilienzzertifikat. Der nächste wertvolle Schritt ist die Widerlegung oder Bestätigung seiner engsten Sicherheitsverträge in isolierten Experimenten.
