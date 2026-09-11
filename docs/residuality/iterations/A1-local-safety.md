# A1 — Von impliziten Annahmen zu expliziten Übergängen

Input: S001–S160, A0-Vertrag, erste Inzidenzmatrix. Entwurfsrevision, keine Implementierung. Erweiterungen C01, C04, C09, C12, C13, C25, C27, C31.

## Revidierte Architektur

```text
Command
  → aktuelle Actor-/Scope-Prüfung
  → Admission (globale Kosten/Queue + Scope-Fairness)
  → Taskintention mit Zielagent und verwendbarer Input-/Kontextrevision
  → transaktionaler Übergang / Event / Attempt-Intent
  → begrenzter Plugin-Aufruf
  → Ergebnis-Evidenz, nie automatisch Taskqualität

Runtime-Beobachtung
  → ausreichende Identität AND autoritativ AND lebendig AND frisch
  → erst dann Session-Promotion
Unklar / veraltet / fremde Identität
  → disconnected + Lease bleibt + lesbarer offener Klärungsbedarf
```

### Die Änderungen und ihre Matrixbegründung

| Änderung | Auslösende Karten | Erhaltener Residual / neue Regel | Preis / verworfene Alternative |
|---|---|---|---|
| A1-D1: Evidence Gate | S031–S040, S062–S063 | R01: Identität, Autorität, Aktualität und Lebendigkeit getrennte notwendige Prädikate. Eine unbekannte Antwort gibt keine Lease frei. | Mehr blockierte Arbeit statt optimistischer Adoption oder Freigabe per Timeout. |
| A1-D2: drei unterschiedliche Autorisierungen | S001, S071–S074, S083 | R02/R09: Task annehmen, Promptversuch genehmigen und Außenwirkung genehmigen sind nicht dasselbe. Resume erhält Actor und konkrete Attempt-Identität. | Weitere Fakten statt einer globalen `approved`-Bool. |
| A1-D3: Arbeitspaket | S061, S075, S101, S111–S116 | R05: Empfänger, Auftragsversion, sichere Contentreferenzen und tatsächlicher Kontext werden Bestandteil der Run-Evidenz. | Keine stillen Kontext-/Modellwechsel unter gleichem Etikett. Kein Full-Transcript-Archiv. |
| A1-D4: begrenzte Integration und Admission | S041–S047, S060, S108, S141–S146 | R06/R07: Framing-, Output-, Laufzeit-, Queue- und Restartgrenzen plus globale Kosten-/Aufnahmegrenze. Gleiche Policy für Agent- und Process-Executor. | Bewusstes Warten und Ablehnen statt unbeschränktem Retry. |
| A1-D5: explizite Geschäftszeit | S021–S025, S030 | R13: Occurrence-ID, DST-Regel, Catch-up-Cap und Verfallsentscheidung sind Vertragsdaten, keine emergente Cron-Eigenschaft. | Kein erratenes Nachholen alter externer Aktionen. |
| A1-D6: Verifikation getrennt halten | S035, S078, S104–S110 | R14: Workerabschluss, Qualität und Dispatch bleiben drei Aussagen. Fixture-Gates statt LLM-Judge für automatisierte Benchverdicts gemäß ADR 0020. | Qualitätsprüfung kostet Ressourcen und löst nicht jede Fachfrage. |
| A1-D7: Liefer-/Cutover-Vertrag | S153–S159 | R16/R03: bestehender Prototyp bleibt bis genehmigtem Cutover alleiniger Besitzer seiner Live-Domäne. Replay-Fähigkeit wird nicht aus bloßer Tabellenexistenz abgeleitet. | Ein expliziter Übergang statt semantisch unehrlichem Dual-Write. |

## Matrixprüfung nach A1

Auf denselben 160 Karten: **41 vollständig benannt, 94 teilweise, 25 ohne benannte Obligation**; davor 9/70/81. Keine dieser Zahlen behauptet bestandene Tests.

Das häufigste gemeinsame Residue-Paar ist R02/R09 mit 12 Karten: Wirkung und Befugnis sind eng gekoppelt, dürfen aber gerade deshalb nicht in einem Status verschwinden. R06/R07 teilen sechs Karten: lokale Plugin-Grenzen ohne globale Admission reichen nicht.

## Warum A1 nicht genügt

- S015: Restore kann alle nach dem Snapshot entstandenen Versuche vergessen. Das Evidence Gate allein erkennt fehlende Vergangenheit nicht.
- S065: Zwei Maschinen können beide einen gültigen lokalen Lock halten. Local-only Single-Writer ist kein globales Fencing.
- S027/S064/S083: Gepinnte Intention darf veraltete Berechtigungen nicht verewigen.
- S045/S011/S091: Safe Mode, menschliche Freigaben und Journal benötigen selbst Ressourcen und einen unabhängigen Bootstrap.
- S006/S016/S017: DB-Backup, Content-Retention und Schlüssel-Recovery sind verschiedene Voraussetzungen.

**Input für A2:** gemeinsame Recovery-Epoche, unabhängiger Kontrollpfad, aktuelle Effektpreconditions und definierte menschliche/physische Recovery-Menge. Die zusätzlichen Organisations-/Wachstumskarten S161–S180 prüfen diese Richtung weiter.
