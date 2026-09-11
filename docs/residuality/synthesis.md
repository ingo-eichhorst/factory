# Verdichtung: 200 Restfähigkeiten → 16 Residues

## Wie wurde verdichtet?

Jede Stressor-Karte nennt zuerst eine noch mögliche Fähigkeit, nicht sofort ein Produktfeature. Gruppiert wurden Karten, deren Fortsetzung dieselben invarianten Bedeutungen benötigt. Technologiegleichheit allein genügt nicht: zwei Dinge in SQLite können völlig verschiedene Restfähigkeiten haben; ein Residue kann sich umgekehrt über Kernel, Plugin und Operator erstrecken.

Beispiele:

- S001, S051, S081 und S090 unterscheiden sich als Strom-, Netz-, Vertrags- und Retentionausfall. Gemeinsam bleibt die **durable Effektabsicht**, während der Ausgang unbekannt sein kann → R02.
- S031, S032, S034 und S040 zerstören verschiedene Beobachtungsannahmen. Gemeinsam bleibt die **evidenzgebundene Belegung**, sofern Unsicherheit keine Lease freigibt → R01.
- S006, S017, S155 und S199 zeigen, dass eine DB-Kopie nicht reicht. Die Wiederaufbaubarkeit umfasst Ausfalldomäne, Schlüssel und Softwarelesbarkeit → R11 zusammen mit R16.
- S104, S110, S184 und S200 greifen den Maßstab an, nicht nur den Worker. **Prüfbare Ergebnisqualität** braucht Herkunft und Grenzen des Urteils → R14; eine allgemeine Wahrheitsgarantie wird nicht daraus.

Alle Einzelzuordnungen stehen nachvollziehbar in [stressors.csv](stressors.csv) und [traceability.csv](generated/traceability.csv). Die folgenden Namen sind Merkhilfen; die vollständige Definition mit Kosten und Grenzen steht in [residues.csv](residues.csv).

| Cluster | Residues | Warum nicht eins? |
|---|---|---|
| Wissen über laufende Arbeit | R01 Belegung, R02 Effektabsicht, R03 Mutationsautorität | Prozessidentität, externe Wirkung und Writerautorität fallen unabhängig voneinander aus. |
| Bedeutung über Zeit | R04 Replay, R05 Arbeitspaket, R13 Zeitbedeutung | Historische Rekonstruktion, portable Intention und heutige Gültigkeit sind verschiedene Zusagen. |
| Kontrollierter Restbetrieb | R06 Integration, R07 Kapazität, R08 Kontrolle | Isolierter Prozess ist weder globales Ressourcenlimit noch garantiert erreichbare Diagnose. |
| Legitimes Handeln | R09 Befugnis, R10 menschliche Steuerung | Authentisierung/Freigabe braucht einen technisch bindbaren Vertrag und real existierende legitime Personen. |
| Fortbestand und Datenhaftung | R11 Recovery, R12 Datenhaftung, R16 Weiterentwicklung | Wiederherstellbarkeit, zulässige Retention und lesbare Softwareformate können kollidieren. |
| Grenzen der Aussage | R14 Qualität, R15 Einsatzgrenze | Fachliche Wahrheit ist nicht Sandbox-Sicherheit; beide dürfen nicht aus grünen Taskzuständen folgen. |

Die anfänglichen sechs groben Hypothesen der ersten Analyse wurden **aufgespalten**, nicht verworfen: der Ressourcenrest wurde z.B. in Integrationslokalität, globale Kapazität und menschliche Steuerung präzisiert. Der große Recoveryrest trennte sich in Bedeutung, Wiederaufbau, Datenhaftung und langfristige Lesbarkeit. Trotzdem bleiben 16 deutlich weniger als 200.

## Was die Matrix beiträgt

- R09 besitzt 48 direkte Inzidenzen, R05 40 und R16 38. Diese sind breite Reviewflächen, nicht automatisch die ersten drei Implementierungsaufgaben.
- R02/R09 teilen 16 Karten: eine globale `approved`-Bool wäre gerade an der dichtesten Kopplung zu grob.
- R06/R07 teilen acht Karten: per-Scope-Subprozesse ohne globalen Deckel können die gemeinsame Maschine zerstören.
- R08/R03 haben jeweils zehn direkte Karten, aber viele bedingte Folgeabhängigkeiten. Häufigkeitsranking allein würde den Kontrollpfad und Mutationseigentümer unterschätzen.
- Die Matrix zeigt **wo** dieselben Restfähigkeiten gebraucht werden. Warum ihre konkrete Konstruktion tragfähig sein soll, steht erst in A1–A3 und muss durch T01–T17 angegriffen werden.

## Zielkonflikte und bewusst verworfene Vereinfachungen

| Spannung | Entscheidung im Entwurf | Preis und verbleibender Konflikt |
|---|---|---|
| R01 Sicherheit vs. R07 Durchsatz | Unklare Lease halten, Admission begrenzen | Ein dauerhaft blockierter Workspace wird nicht per TTL „sicher“. |
| R04 vollständiger Replay vs. R12 Löschung | Inhaltsklassen und explizite Evidenzgrenzen | Nicht beide absoluten Zusagen gleichzeitig anbieten; C36 bleibt konkret zu entscheiden. |
| R05 Reproduzierbarkeit vs. R09 aktuelle Policy | Historie pinnen, heutige Erlaubnis neu prüfen | Eine reproduzierbare alte Aktion kann heute verboten sein. |
| R09 Freigabequalität vs. R10 Menschenkapazität | Lesbare begrenzte Entscheidungspakete | Weniger Dialoge dürfen nicht versteckte Sammelfreigaben bedeuten. |
| R06 Scopeisolation vs. R07 globale Ressourcen | Lokale Aktivierungen plus globales Budget | Kein unbeschränktes Process-per-Scope unter wachsender Registry. |
| R03 Exklusivität vs. Verfügbarkeit | Single-active und echtes Teilnehmerfencing wo verfügbar | Kein Active-active allein aus zwei UUIDs oder zwei lokalen Locks. |
| R08 Kontrollverfügbarkeit vs. ein Mutator | Bundled Safe Mode und ausdrücklich read-only Offline-Doctor | Kein privilegierter Notfallwriter, keine direkte Herdr-Backdoor im Client. |
| R11 Recovery vs. R12 Vertraulichkeit | Klassifiziertes genehmigtes Recovery-Set, Keys separat | Mehr Kopien vergrößern auch Datenhaftung. |
| R14 Prüfqualität vs. Kosten | Domänengates und Instrumenttransparenz | Ein LLM-Judge oder mehr identische Prüfer ersetzt keine unabhängige Wahrheit. |
| R16 Anpassungsfähigkeit vs. Kernelgröße | Verträge in bestehende Grenzen integrieren | Kein neuer Dienst oder Plugin für jede Stressorkarte. |

## Kein Universalresidue

„Alles stoppen und Backups haben“ wäre zwar eine kleine Tabelle, aber keine ausreichende Architektur: Manche Aufgaben müssen nachvollziehbar weiterlaufen, ein unklarer Effekt braucht externe Evidenz, und vollständiger Datenverlust lässt nichts wiederaufbauen. Umgekehrt ist „alles automatisch reparieren“ weder sicher noch glaubwürdig.

Der Entwurf hält deshalb pro Karte einen dominanten Restbetriebsmodus und eine explizite Verlust-/Entscheidungsgrenze fest. 22 Karten behalten in A3 fehlende Obligationen. Auch bei den übrigen 178 gilt nur, dass eine Entwurfsantwort benannt wurde, nicht dass die Welt sie einhält.
