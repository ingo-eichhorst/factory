# A4 — unabhängig bemerken, ehrlich messen, rechtzeitig begrenzen

**Historischer Entwurf:** Die aktuelle [Zielarchitektur A7](../a7/zielarchitektur.md) folgt der vollständigen zustandsbasierten Herleitung aller 350 Stressoren. Die hier bewahrte Verdichtung auf 17 Kategorien ist keine endgültige Residue-Menge. A7 präzisiert insbesondere eigenständige Erhaltungs-, Teilnehmer-, Modul- und Einsatzgrenzen; vorhandene ADRs werden weiterhin nicht automatisch geändert.

**Status: revidierter Architekturvorschlag, nicht implementiert oder durch Produkttests bestätigt.** A4 ergänzt [A3](../architecture-A3.md); nicht erwähnte Verträge gelten im Entwurf weiter. Bestehende ADRs werden dadurch nicht still geändert. Die 200 alten Karten bleiben historische Baseline, die 150 neuen Karten und 50 Suchstrategien sind [hier](generated/stressors.md) nachprüfbar.

## 1. Was hat sich durch diese Runde geändert?

| Auslösende Stressoren | Daraus erhaltene Fähigkeit | Konkrete Entscheidung | Nachweisidee |
|---|---|---|---|
| S201 Rechner stromlos; S204 gleicher Stromkreis; S213 alter Heartbeat | **Neu R17:** Ausfall wird auch ohne Factory bemerkbar | C37: unabhängiger Beobachter mit eigenem Frische- und Ausbleibevertrag | T18 |
| S207 Monitor tot; S208 Alarm nicht gelesen; S209 Mute endet nicht | R17: die Meldung hat einen tatsächlich nutzbaren Weg | C38: gesamte Alarmkette, endliche Wartung, unabhängige Gegenprüfung | T19 |
| S210 Healththread grün/Kernel tot; S211 nur Reads funktionieren | R08: Zustand bleibt ehrlich erkennbar | C39: mehrere konkrete Healthbefunde statt eines pauschalen `healthy` | T20 |
| S219 acht Teilfristen; S220 zwei Tage Queue; S231 Antwort nach Retention | R13: Zeit und Wirkung bleiben noch sinnvoll zuordenbar | C40: gemeinsames Restbudget, getrennte fachliche Gültigkeit, späte Antwort gehört zur alten Operation | T21 |
| S225 zehntausend Antworten; S226 doppelter Rechnungsversand; S237 Retrymultiplikation | R07: begrenzte nützliche Kapazität | C41: begrenzter Fan-out und globales Retrybudget, kein spekulativer Effektduplikat | T22 |
| S222 nur fertige Tasks gezählt; S223 Lasttest pausiert beim Hänger; S224 unbelegtes Extremperzentil | R07: Kapazität auf ehrlicher Datenbasis steuern | C42: Queue, Ablehnung, Abbruch und offene Fälle neben abgeschlossenen Zeiten messen | T23 |
| S249 Replay dauert Wochen; S285 Einheitenwechsel; S288 Rollback nach Außenwirkung | R16: reale statt nur nominelle Änderbarkeit | C43: Versions-, Kaltstart-, Volumen- und Wirkungsgegenproben im Migrationsvertrag | T24 |
| S261 falsches Scope-Credential; S262 Aliaswechsel; S265 gefälschte Logfreigabe | R09: begrenzte tatsächliche Handlungsbefugnis | C44: aufgelöstes Ziel prüfen, Inhalt nie mit Actorbefugnis verwechseln | T25 |
| S216 Label-Explosion; S276 vertrauliche Heartbeatdaten; S277 Rohkontext im Export | R12: weniger Datenhaftung auch durch Monitoring | C45: minimale erlaubte Felder, begrenzte Labels und definierte Retention | T26 |
| S304 falsche Instanz im Handytext; S306 niemand erreichbar; S307 Alarmflut | R10: endliche menschliche Reaktionsfähigkeit | C46: klarer zuständiger Empfänger, verständlicher Text, Vertretung und begrenzte Belastung | T27 |
| S227 fehlender Pflichtteil; S300 falsche Einheit; S335 optimierter Scheinerfolg | R14: fachlich prüfbare Ergebnisse | C47: Bedeutung, Vollständigkeit, Gültigkeit und Wirkungsrisiko prüfen | T28 |
| S269 gemeinsamer kompromittierter Updater; S286 globale Pluginmigration; S312 gleicher Cloudunterbau | R16: begrenzter Änderungs- und Ausfallradius | C48: wirkliche gemeinsame Abhängigkeiten inventarisieren und prüfen | T29 |

Diese Tabelle zeigt ausgewählte Auslöser. Die [vollständige Zuordnung aller 350 Karten](generated/traceability.csv) verbindet jede Karte mit Residue, Regel, Revision und vorgeschlagenem Experiment. R01–R16 bleiben erhalten; **es entsteht nur ein neues Residue und kein neuer Dienst pro Qualitätskriterium**.

## 2. Der neue äußere Beobachtungsring — C37/C38

```text
Factory-Rechner / Strom / lokales Netz
┌─────────────────────────────────────────┐
│ Ein Daemon: Zustand und begrenzte Probe │
│ Explizit aktiviertes Telemetrieplugin   │
└─────────────────┬───────────────────────┘
                  │ minimale frische authentifizierte Meldung
                  ▼
       Unabhängiger externer Beobachter
       eigene Uhr + letzter gültiger Befund
                  │ Ausbleiben / Zustand / Wiederkehr
                  ▼
       Genehmigter unabhängiger Alarmweg
                  ▼
       Betreiber → legitimierte Vertretung

Separater Prüfweg kontrolliert Beobachter UND Zustellkette.
Keiner dieser Beobachter erhält dadurch Reparatur- oder Dispatchrecht.
```

### Was der Vertrag festlegt

- **Ausfalldomäne:** Der Beobachter läuft nicht auf dem Factory-Rechner und nicht nur als zweiter Prozess am gleichen Router. Strom, Netz, Identitätsanbieter, Zahlung, Lieferkette und Alarmzustellung werden auf gemeinsame Abhängigkeiten geprüft. Nicht jeder gemeinsame Anbieter ist verboten; die verbleibende gemeinsame Ausfallgrenze muss ausdrücklich benannt sein.
- **Gegenstand:** Host-Erreichbarkeit, aktuelle Kernelreaktion und fachlicher Fortschritt sind verschiedene Befunde. Ein OS-Heartbeat darf weiter melden, während Factory ausgefallen ist — er darf nur nicht als Factory-Gesundheit gelten.
- **Frische:** Minimale pseudonyme Installationskennung, Inkarnation, Sequenz, Beobachtungsebene und begrenzter Zustandsbefund. Authentifizierung über den Secretvertrag. Ein lediglich bislang unbekanntes Sequenzfeld beweist noch keine frische Entstehung.
- **Verzögerte Pulse:** Der Vertrag braucht eine von der externen Zeitbasis begrenzte Frischeprüfung, beispielsweise eine kurzlebige externe Challenge, die durch eine neue begrenzte Kernelprobe beantwortet wird. Alte gecachte oder aufgestaute Pulse erfüllen diese Challenge nicht. Gleichwertige Verfahren sind möglich, müssen dieselben Gegenproben tragen. Eine unbefristet gültige signierte Meldung genügt nicht.
- **Eigene Zeit:** Ausbleiben und Challengeablauf werden durch den externen Beobachter bewertet, nicht durch die Uhr des ausgefallenen Rechners. Nach einem Restore des Beobachters gelten dessen alte Frische- und Muteannahmen ebenfalls nicht ungeprüft weiter.
- **Alarmsemantik:** „Kein aktueller Kernelbefund seit …“ statt „Mac definitiv kaputt“. Beobachtungsausfall, Alarm erzeugt, Zustellbestätigung, menschliches Ack und behoben sind verschiedene Fakten. Ein später grüner Host macht einen unbekannten Businessvorgang nicht erledigt.
- **Rückkehr:** Ein altes nachgeliefertes Paket beendet keinen Alarm. Frische Wiederkehr wird gesondert gemeldet; stabiler Zustand und erneute Dispatchbefugnis werden getrennt geprüft. Flapping darf gebündelt werden, aber nicht in ewiger Stummschaltung verschwinden.
- **Metaüberwachung:** Eine unabhängig erwartete Ende-zu-Ende-Probe muss auch den Ausfall des Beobachters oder Zustellwegs bemerkbar machen. Ein Check, der ausschließlich im ausgefallenen Monitor selbst läuft, erfüllt das nicht. Möglich ist ein unabhängiger Watchdogdienst oder ein gesonderter Prüfprozess außerhalb dessen Ausfalldomäne; daraus folgt keine Pflicht zu einer großen Monitoringplattform.
- **Keine endlose Rekursion:** Es wird eine benannte Zahl realer unabhängiger Wege geprüft, kein „Monitor für jeden Monitor“ bis ins Unendliche. Verschwinden alle Beobachter, Kommunikationswege und Empfänger, ist Alarmierung unmöglich (S345).

### Vor unbeaufsichtigtem Betrieb zu vereinbaren

| Parameter | Bedeutung |
|---|---|
| Beobachtungsintervall und Frischefenster | Wie oft erwarten wir welchen aktuellen Befund? |
| Ausbleibegrenze und Prüftakt | Ab wann wird ein fehlender Befund als Alarmzustand geführt? |
| Zustell- und Eskalationsziel | Wen erreicht welcher genehmigte Weg, und wann folgt Vertretung? |
| Wartungsgrenze | Wer darf wie lange welche Alarmierung ausdrücklich aussetzen? |
| Probeintervall | Wie oft testen wir die gesamte Kette samt tatsächlichem Empfänger? |
| Gemeinsame Ausfallgrenze | Welche regionalen, Account- oder Providerkombinationen bleiben ungeschützt? |

Die Erkennungszeit ergibt sich aus Ausbleibegrenze und externem Prüftakt; Zustellung und menschliche Reaktion kommen hinzu. Daraus folgt **kein festes RTO**, solange deren Voraussetzungen nicht vereinbart und getestet sind. Beispielwerte in einer Testfixture wären keine Produktionszusage.

**Freigabegrenze:** Anbieterwahl, externe Telemetrie, Empfänger, Inhalte, Kosten und regelmäßige Nachrichten brauchen explizite Genehmigung. Diese Analyse richtet nichts ein. Die Aktivierung bleibt scopegebunden. Ohne Aktivierung gibt es kein externes Alarmversprechen, der lokale Kern muss aber start- und inspizierbar bleiben. Ein fehlender Monitor ist kein automatischer Kill-Switch und berechtigt niemals zu Failover, Leasefreigabe oder erneutem Geschäftsversand.

## 3. Gesundheit als konkrete Evidenz — C39/C45

Eine transportneutrale, begrenzte Diagnoseantwort des Kernels soll mindestens unterscheiden können:

1. Reagiert der Kernel auf eine **aktuelle** Anfrage?
2. Welche Persistenzevidenz liegt tatsächlich vor und wie alt ist sie?
3. Gibt es fällige Arbeit, welche ist blockiert, und warum?
4. Welche Budgets sind erschöpft oder Integrationen eingeschränkt?
5. Wie alt und vollständig ist die Beobachtung selbst?

Ein frischer read-only Zugriff beweist keine aktuelle Schreibfähigkeit. Ein historischer Commitcursor beweist nur damaligen Fortschritt. Eine mutierende synthetische Prüfung wäre ein gesondert genehmigter Vorgang; sie darf weder versteckt Businessaktionen auslösen noch Replay verunreinigen. Leerlauf ohne fällige Arbeit ist kein Fehler.

Health, Metriken und Traces sind **abgeleitete begrenzte Sichten**, keine neue kanonische Wahrheit. Telemetrieausfall darf Kerncommits nicht unbegrenzt blockieren. Wegwerfbare Messdaten dürfen unter Last entfallen, nötige Ereignis-/Operationsevidenz dagegen nicht. Keine Kundennamen, Secretwerte, Rohprompts oder unbeschränkten Run-/Tokenlabels in externen Metriken. Dropcounter und Beobachtungslücken bleiben sichtbar.

## 4. Nicht nur schneller — rechtzeitig und noch gültig — C40/C41/C42

### Ein Budget statt ständig neuer Fristen

Ein Auftrag besitzt eine explizite Bedeutung seiner Frist: technische Wartezeit, fachliche Eingabegültigkeit und Freigabeablauf sind getrennt. Für einen begrenzten Aufruf zählt **Queue + lokale Arbeit + alle nötigen Unteraufrufe**. Unteraufrufe erhalten nur das verbleibende zulässige Budget, nicht jedes Mal eine neue volle Frist.

Lokale monotone Uhr und externe Kalenderzeit sind nicht beliebig zwischen Hosts verrechenbar. Adapter müssen Budgetübertragung und Ungewissheit konservativ behandeln. Suspend- und Neustartverhalten gehört zum Zeitvertrag. Bei unklarer aktueller Gültigkeit wird keine neue Außenwirkung legitimiert.

### Das Ende des Wartens ist nicht das Ende der Wirkung

- Cancel und Timeout beenden zunächst die eigene Warte- oder Startbefugnis.
- Eine schon angenommene Außenwirkung kann trotzdem eintreten.
- Späte Antworten werden an die ursprüngliche Operation gebunden und mit heutiger Eingangszeit als späte Evidenz erfasst; alte Events werden nicht rückwirkend geändert.
- Ist die Zuordnung zulässig nicht mehr vorhanden, wird die Antwort begrenzt als unzuordenbar behandelt, nicht zu einer neuen Aufgabe oder Freigabe umgedeutet.
- Maximale akzeptierte Verzögerung, Teilnehmer-Idempotenzretention und lokale Datenretention müssen zusammen betrachtet werden. Fehlt dieser gemeinsame Vertrag, ist „unbekannt und nicht automatisch wiederholen“ ein möglicher dauerhafter Ausgang.

**Extrembefund S347:** Unbegrenzte mögliche Teilnehmerlatenz, endliche Erinnerung und garantiert sichere automatische Wiederholung lassen sich nicht gleichzeitig zusagen. A4 löst das nicht durch einen längeren Timeout.

### Tail-Toleranz ohne doppelte Geschäftsaktionen

Ein gemeinsames Retry-/Fan-out-Budget ergänzt die bestehenden Aufnahme- und Kostenlimits. Wiederholungen benötigen Fehlerklassifikation und begrenzten Jitter. Auch SDK-interne Versuche zählen; ein undurchsichtiger Teilnehmer bietet keine belegte Gesamtgrenze.

Spekulative Zweitanfragen zur Latenzsenkung sind **nicht** standardmäßig für externe Effekte erlaubt. Für eindeutig read-only oder tatsächlich nachgewiesen unschädlich deduplizierte Operationen wäre eine explizite begrenzte Regel möglich. Weder Backup noch langsame Antwort begründet eine solche Erlaubnis. Unbekannte Workspacebelegung bleibt trotz Kapazitätsdruck geschützt.

### Ehrliche Zeitmessung

Erfasst werden mindestens angebotene, zugelassene, abgelehnte, wartende, abgebrochene und abgeschlossene Arbeit sowie Wiederholungsanzahl. Queuealter, Ausführungszeit und Ende-zu-Ende-Zeit werden nicht verwechselt. Offene Fälle sind **noch nicht vollständig beobachtete Dauern**, keine schnellen Erfolge und keine erfundenen fertigen Werte.

Histogramme brauchen begrenzte Klassen, Beobachtungsfenster und Samplezahlen. Ein Test, der während eines Hängers keine neue Nachfrage erzeugt, unterschätzt offene Last. Eine Behauptung über p99.999 benötigt eine passende Messbasis — der Name allein genügt nicht. Kein gemessener endlicher Tail liefert eine harte Echtzeitgarantie.

## 5. Qualitätsregeln ohne neuen Großkern — C43/C44/C46/C47/C48

- **Security:** Aktuell aufgelöste Zielidentität, Scope, Actor und Credentialbindung gemeinsam prüfen. Ein signiertes Plugin kann bösartig sein. Promptinhalt, Fehlerlog und importierte Task-ID verleihen keine Autorität. Same-User-v1 bleibt keine Sandbox; C33 bleibt offen für ein separates feindliches Einsatzmandat.
- **Wartbarkeit:** Nicht nur aktuelle Version gegen aktuelle Version testen, sondern erlaubte Client-/Plugin-/Storekombinationen, Kaltstart, temporären Speicherbedarf und alte Außenwirkungen beim Rollback. Ein verlorener Maintainer oder Cloudaccount muss im Wiederaufbauplan vorkommen.
- **Änderungsradius:** Kritische gemeinsame Abhängigkeiten einschließlich Strom, Accounts, Parser, Updater und Provider dokumentieren. Eine optionale Scopeintegration darf keine stillschweigende globale Startpflicht erzeugen. Größere Einsatzgrenzen verlangen einen ausdrücklichen Architekturentscheid, keine weitere Ausnahme im Kern.
- **Menschliche Bedienbarkeit:** Alarmtexte zeigen eindeutig betroffene Instanz, Unsicherheit und sichere nächste Schritte, nicht nur Farbe oder gekürzte Namen. Bereitschaft, zugelassene Vertretung, Belastungsgrenze und Muteablauf gehören vor den unbeaufsichtigten Betrieb. Reales Verständnis ist durch Nutzerprobe zu prüfen, nicht allein durch CSVvalidierung.
- **Fachliche Qualität:** Einheit, Zielperson, Pflichtvollständigkeit, Datenalter und Wirkungsrisiko gehören zu Akzeptanzkriterien. Ein schneller falscher Bericht zählt nicht als brauchbarer Durchsatz. C35 bleibt offen, wo kein tragfähiger unabhängiger Fachmaßstab vorliegt.

Kein neuer Message-Store, kein allgemeiner Workflowengine, keine Clusterpflicht und kein Wechsel der kanonischen Datenbank folgen daraus. Der Kernel behält generische Verträge, die externen Anbieter bleiben hinter Plugins bzw. beim unabhängigen Operator-Dienst. Factory-eigene Dateien bleiben unter der Instanz-`.factory/`; dieser Vorschlag führt keinen zusätzlichen Schreibpfad außerhalb davon ein.

## 6. Was ist bewusst noch offen?

C33 harte Isolation, C34 unabhängige Wirkungsevidenz nach Historienverlust, C35 tragfähiger Fachmaßstab und C36 konkrete Rechtskonfliktentscheidung bleiben offen. Sie betreffen im erweiterten Korpus **39 Karten**. Manche anderen Karten haben alle Regeln benannt und dennoch als Restbetrieb Verlust oder Nicht-Unterstützung: Das ist keine gelöste Verfügbarkeit.

Weitere noch festzulegende Parameter sind tatsächliche Monitor-/Alarmwege, Fristen, Kapazitäten, Serviceklassen, Retention und Bereitschaft. Ein in A4 benannter Vertrag ist noch keine konfigurierte oder getestete Betriebsfähigkeit. Die [gemeinsame Auswertung](generated/summary.md) darf deshalb nicht als Erfolgsquote gelesen werden.

## 7. Ratifikation und nächste Prüfungen

1. **Zuerst T18–T20/T27/T29 spezifizieren:** Totalausfall, falsches Grün, alte Pulse, Monitor-/Alarmwegverlust und erreichbare legitime Vertretung. Für erste Gegenproben genügen isolierte Fakes. Der reale Ende-zu-Ende-Drill bleibt eine eigene genehmigte Aufgabe.
2. **T21–T23 gegen Extremlatenz und Last:** getrennte Annahme-/Antwortzeit, Retentionende, offene Dauerverteilung, Retrymultiplikation und Kontrollreserve. Nicht einfach Timeoutwerte erhöhen.
3. **T24–T26/T28 als Qualitätsgates:** Kompatibilität/Kaltstart, Autoritätsverwechslung, minimale Telemetrie und fachlich falscher Erfolg. Die früheren T01–T17 bleiben nötig.
4. **In bestehende Verträge einordnen:** ADR 0002/0007 für Diagnose-/Pluginvertrag und Optionalität, ADR 0003/0019 für späte Evidenz/Restore, ADR 0020 für getrennte Mess- und Qualitätsbedeutung. Externe Überwachung braucht zusätzlich einen expliziten Betriebsvertrag samt Freigabe. Diese Dokumentation ratifiziert oder implementiert sie nicht automatisch.

**Leitsatz A4:** Factory soll seine eigene Ungewissheit nicht verstecken — und der Betreiber soll nicht von einem ausgefallenen Factory erfahren müssen, dass Factory ausgefallen ist.
