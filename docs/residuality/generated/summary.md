<!-- Generated exclusively by docs/residuality/build_matrices.py. -->
# Matrixauswertung

**Nur syntaktische Entwurfsobligationen; keine bestandenen Resilienztests.**

Korpus: 200 Stressoren; 16 Residues; 443 direkte Inzidenzen.

| Revision | Alle zugeordneten Obligationen benannt | Teilweise | Keine |
|---|---:|---:|---:|
| A0 | 9 | 89 | 102 |
| A1 | 41 | 123 | 36 |
| A2 | 120 | 76 | 4 |
| A3 | 178 | 22 | 0 |

`benannt` bedeutet nur: Die zugewiesenen Vertragsantworten existieren im Entwurf.
Es bedeutet weder beherrscht noch implementiert. `offen` eingeführte Kontrollen bleiben fehlend.
Auch vollständig benannte Szenarien können Verlust oder einen bewussten Betriebsstopp bedeuten.

## Restbetriebsarten (unabhängig vom Obligationenstatus)

- degradiert: 40
- grenze: 20
- halten: 106
- offen: 20
- verlust: 14

## Inzidenz je Residue

| Residue | Direkte Stressoren |
|---|---:|
| R09 — Begrenzte Handlungsbefugnis | 48 |
| R05 — Portables Arbeitspaket | 40 |
| R16 — Wartbare Weiterentwicklung | 38 |
| R02 — Durable Effektabsicht | 33 |
| R04 — Replaybare Bedeutung | 31 |
| R06 — Lokalisierter Integrationsausfall | 30 |
| R10 — Endliche menschliche Steuerung | 29 |
| R11 — Wiederaufbaubare Installation | 29 |
| R01 — Evidenzgebundene Belegung | 28 |
| R07 — Begrenzte produktive Kapazität | 28 |
| R13 — Explizite Zeitbedeutung | 24 |
| R12 — Begrenzte Datenhaftung | 23 |
| R14 — Prüfbare Ergebnisqualität | 22 |
| R15 — Ehrliche Einsatzgrenze | 20 |
| R03 — Eindeutige Mutationsautorität | 10 |
| R08 — Minimaler Kontrollbetrieb | 10 |

## Häufigste gemeinsame Inzidenzen

Keine statistische Korrelation und keine gemessene Ausfallabhängigkeit.

- R02 / R09: 16 gemeinsame Stressoren
- R05 / R16: 10 gemeinsame Stressoren
- R11 / R16: 9 gemeinsame Stressoren
- R04 / R16: 9 gemeinsame Stressoren
- R04 / R11: 9 gemeinsame Stressoren
- R06 / R07: 8 gemeinsame Stressoren
- R05 / R14: 8 gemeinsame Stressoren
- R09 / R15: 7 gemeinsame Stressoren

## Weiterhin fehlende Obligationen in A3

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
