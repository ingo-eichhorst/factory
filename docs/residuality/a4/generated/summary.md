<!-- Generated exclusively by docs/residuality/a4/build.py. -->
# A4: gemeinsame Auswertung

350 Stressoren (200 historisch + 150 neu), 50 neue Suchstrategien, 17 Residues.
740 direkte Inzidenzen; 1750 Revisionsbewertungen.

**Nur Benennung zugeordneter Entwurfsregeln, keine Erfolgsquote oder unabhängige Validierung.**
Alle Revisionen werden hier auf denselben 350 Karten bewertet. Die alte 200er-Auswertung bleibt unverändert.
Der additive Bewertungsmechanismus steigt konstruktionsbedingt. Er misst keine tatsächliche Verbesserung.

| Revision | Alle Regeln benannt | Teilweise | Keine |
|---|---:|---:|---:|
| A0 | 10 | 113 | 227 |
| A1 | 46 | 171 | 133 |
| A2 | 132 | 156 | 62 |
| A3 | 197 | 123 | 30 |
| A4 | 311 | 39 | 0 |

## Direkte Residue-Zuordnung

| Residue | Baseline | Neu |
|---|---:|---:|
| R01 — Evidenzgebundene Belegung | 28 | 7 |
| R02 — Durable Effektabsicht | 33 | 15 |
| R03 — Eindeutige Mutationsautorität | 10 | 6 |
| R04 — Replaybare Bedeutung | 31 | 3 |
| R05 — Portables Arbeitspaket | 40 | 8 |
| R06 — Lokalisierter Integrationsausfall | 30 | 8 |
| R07 — Begrenzte produktive Kapazität | 28 | 36 |
| R08 — Minimaler Kontrollbetrieb | 10 | 14 |
| R09 — Begrenzte Handlungsbefugnis | 48 | 20 |
| R10 — Endliche menschliche Steuerung | 29 | 25 |
| R11 — Wiederaufbaubare Installation | 29 | 12 |
| R12 — Begrenzte Datenhaftung | 23 | 16 |
| R13 — Explizite Zeitbedeutung | 24 | 27 |
| R14 — Prüfbare Ergebnisqualität | 22 | 21 |
| R15 — Ehrliche Einsatzgrenze | 20 | 7 |
| R16 — Wartbare Weiterentwicklung | 38 | 37 |
| R17 — Unabhängige Ausfallerkennung | 0 | 35 |

## Weiter offene Verpflichtungen auf dem Gesamtkorpus

- S090: C34 — Kein technischer Beweis des Ausgangs nach Retentionverlust
- S098: C35 — Unabhängigkeit braucht organisatorische Realität
- S104: C35 — Gemeinsame fachliche Blindstellen bleiben
- S110: C35 — Keine allgemeine Kompetenzgarantie aus endlichem Korpus
- S117: C33 — Fremdrepositories brauchen Isolation vor Ausführung
- S121: C33 — Keine Geheimnisgarantie nach Same-User-Kompromittierung
- S124: C33 — Hard Isolation ist separates Deploymentprojekt
- S131: C36 — Juristische Abwägung und Replayverlust sind explizit zu entscheiden
- S132: C36 — Kein automatischer Rechtsentscheid
- S137: C36 — Keine technische Umgehung rechtmäßiger Zugriffsanforderungen geplant
- S138: C36 — Historische Kopien und Replay können betroffen sein
- S157: C35 — Testzahl allein misst keine Sicherheitsqualität
- S170: C34 — Automatische Rekonziliation kann endgültig unmöglich werden
- S172: C33 — Mandantenbetrieb benötigt eigenes Isolationsmandat
- S177: C33 — Externer Trustanchor und andere Deploymentgrenze nötig
- S184: C35 — Keine universelle Wahrheitsmaschine aus der Inzidenzmatrix
- S186: C36 — Keine Architektur kann beide absoluten Forderungen erfüllen
- S189: C36 — Eigentums- und Zugriffsentscheidung benötigt legitime externe Autorität
- S191: C34 — Ohne unabhängige Teilnehmerhistorie ist Ausgang dauerhaft unentscheidbar
- S194: C36 — Rechtliche Entscheidung kann technische Recovery bewusst begrenzen
- S195: C35 — Kein fachlicher Erfolg ohne tragfähigen unabhängigen Maßstab
- S200: C35 — Diese selbstreferenzielle Karte widerlegt keine unbekannten weiteren Blindstellen
- S231: C34 — Fehlende Teilnehmerhistorie kann den Ausgang dauerhaft unklärbar machen
- S233: C36 — Retention und Aufklärung können unvereinbar sein
- S251: C34 — Unbegrenzte Teilnehmerlatenz und endliche Retention kollidieren
- S267: C33 — Feindlicher Code ist im selben Useraccount nicht sicher eingeschlossen
- S273: C33 — Ohne harte Isolation besteht keine Geheimhaltungszusage
- S274: C33 — Telemetrieminimierung beseitigt geteilte Hardwarekanäle nicht
- S279: C35 — Gemeinsame Blindstellen können unentdeckt bleiben
- S319: C36 — Architektur kann widersprüchliche Pflichten nicht gleichzeitig erfüllen
- S320: C36 — Vertragsprüfung und Rechtsentscheidung bleiben menschliche Aufgaben
- S323: C35 — Ohne legitime Fachentscheidung gibt es keinen tragfähigen Maßstab
- S331: C35 — Eine unbekannt falsche gemeinsame Quelle kann alle Prüfungen täuschen
- S332: C34 — Ohne zusätzliche Evidenz kann der Konflikt dauerhaft unauflösbar sein
- S335: C35 — Kein universeller unabhängiger Wahrheitsmaßstab vorhanden
- S344: C36 — Eine konkrete Rechts- und Eigentumsentscheidung bleibt erforderlich
- S346: C35 — Ohne unabhängige Realitätsevidenz ist der Irrtum nicht erkennbar
- S347: C34 — Unbegrenzte Latenz endliche Erinnerung und sichere Wiederholung sind nicht gleichzeitig garantiert
- S350: C33 C34 C35 — Ein fremder Monitor kann fehlende Heartbeats erkennen aber keine perfekt gefälschte Welt widerlegen

Auch bei vollständig benannten Regeln kann der Zielmodus Verlust oder Nicht-Unterstützung sein.
S345 ist die explizite Grenze: Sind alle Beobachter und Empfänger weg, kommt kein Alarm.
