# A7 Betrieb: S001–S100

Run: `503be985-be47-4d1b-973c-5eec67c9f37a`  
Stand: 2026-09-09. Ausdrücklich beauftragte Folgephase nach Phase 1.

## Ergebnis und Reichweite

Alle 100 neutralen Szenarien wurden aus den eigenen ursprünglichen Trajektorien erneut durchgegangen. `cases.json` enthält genau einen Fall je S001–S100 und konkrete bedingte Zweige. `residues.csv` benennt die jeweils tatsächlich nutzbaren Gegenstände, ihre Nutzer, Voraussetzungen und Grenzen. Es gibt keine vorgegebene oder endgültige globale Residuezahl.

**Dies ist eine Entwurfs- und Gegenbeispielanalyse, kein empirischer Nachweis.** Kein neuer Schutz ist damit als heute implementiert, zuverlässig betrieben oder tatsächlich überlebend bestätigt. Ein Zweig mit einem Schutzobjekt gilt nur unter seinen ausdrücklich genannten Voraussetzungen und der vollständigen Vertragsgrenze seines Kandidaten. Auch ein vorhandener Guard in einem einzelnen historischen Quellpfad belegt nicht den gesamten Zweig.

Die Zweige sind weder gegenseitig ausschließend noch ein Vollständigkeitsbeweis: Eine Email kann real versendet sein, während Factory ihren Ausgang nicht kennt. Ein Task kann korrekte Bytes besitzen und noch keinen akzeptierten Abschluss. `source_states` übernimmt ausschließlich echte IDs der ursprünglichen Einordnung. Bei den neun damals offenen Fällen bleibt diese Liste leer; neue Formulierungen in den Zweigen sind keine nachträglich erfundenen OP-Zustände.

Nur die drei beauftragten Dateien unter `a7/` wurden geschrieben. Keine Produktänderung, neue Konfiguration, Agentenverwaltung, Worktrees, Livefehler oder externen Wirkungen. Keine neuen Peer-A7-Ergebnisse gelesen. Die alten Analysen bleiben eingefroren.

## Lesart der Zustände

- **Halt:** Eine konkrete Freigabe, Ressource oder erforderliche Evidenz fehlt und eine tatsächlich vorausgesetzte Grenze hält die betroffene Handlung an. Der Halt selbst ist kein Residue.
- **Ungewissheit/offen:** Ein Sachverhalt oder Vertrag ist nicht bestimmt. Ein erhaltenes Intentjournal kann dabei nützlich sein, obwohl es den Ausgang nicht beweist. `keines` kann sich auf genau den fehlenden Ordnungs- oder Rekonstruktionsnachweis beziehen, ohne Weltwissen über dauerhafte Vernichtung zu behaupten.
- **Äußerlich erzwungen:** Stromausfall, dauernde externe Last, fehlender Fernzugang oder fortgesetzte konkurrierende Schreiber halten den Zustand aufrecht. Ebenso ein technisch erzwungener Restartzyklus ohne nachgewiesene Attraktion.
- **Vorübergehend:** Endliche gültige Arbeit oder eine begrenzte Übergabe kann bei ausreichendem Dienst enden. Das sagt noch nichts über andere wartende Aufgaben.
- **Abschluss:** Immer den konkreten abgeschlossenen Vorgang lesen. Ein abgewiesener Request, eine materialisierte Occurrence oder eine abgearbeitete alte Observation ist nicht automatisch eine fertig erfüllte Geschäftsaufgabe. Eine belegt angenommene Wirkung kann abgeschlossen und trotzdem falsch autorisiert sein.
- **Verlust:** Die ausdrücklich genannte Eigenschaft fehlt: etwa Originalbytes, richtige Zuordnung, Zustimmungstreue oder Einmaligkeit. Nicht automatisch das gesamte Unternehmen oder der gesamte Datenbestand.
- **Eskalation:** Unbegrenztes Wachstum bis Ressourcenende ist keine belegte stabile Hochlastlage.
- **Attraktorhypothese:** Nur mit benannten Rückkopplungskanten und zusätzlicher Bedingung eines begrenzten wiederkehrenden Regimes. Diese Begrenzung und die Rückkopplungsstärken sind hier nicht gemessen.

`bedingt` bedeutet erhaltene Nutzbarkeit unter den genannten Bedingungen, nicht Wahrscheinlichkeit oder Produktstatus. `teilweise` nennt eine begrenzte weiter nutzbare Struktur, während die betroffene weitergehende Fähigkeit fehlt. Leere Referenzlisten bei `keines` und `ungeklaert` sind absichtlich, nicht unvollständig ausgefüllte Erfolgsfälle.

## Neu konkretisierte Mechanismen

### 1. Ein Wiederanlauf hat mehrere unabhängige Grenzen

S001, S008, S011, S014, S015, S072 und S095 zerfallen nach dem tatsächlichen Schnittpunkt:

1. Fehlgeschlagener Voraussetzungskommit vor Writer: auf dem historischen C1-Pfad keine neue Writerübergabe.
2. Erhaltener Zustellversuch ohne Empfangsbeleg: verbrauchtes Budget bleibt ein eigener Halt.
3. Alter Snapshot ohne späteren Versuch: der fehlende Nachlauf ist nicht durch wiederholtes Lesen desselben Präfixes zu gewinnen.
4. Cancel zwischen Intent und Writer: beide Records können erhalten sein, ohne dass der spätere Writer verhindert wurde. Das gilt nur ohne übergreifende Caller-Serialisierung. Der historische manuelle Writer beweist keine externe Annahme.
5. Alter Clone kann lesbar sein, aber darf nur unter tatsächlich durchgesetzter fehlender Dispatchautorität als nicht sendefähig gelten.

Der Snapshotpräfix, das Zustellbudget und die Clone-Dispatchgrenze sind deshalb verschiedene Gegenstände. Neue Zustellautorisierung löst weder alte Workspacebelegung noch fehlendes Teilnehmerwissen.

### 2. Fehlende Sicht ist nicht fehlende Arbeit

S010, S031–S040, S055, S056, S059, S096 und S100 unterscheiden lokale Ausführung, aktuelle Beobachtung, Bedienzugang und menschliche Entscheidung.

Eine Lease kann verwaltete Wiedervergabe verhindern, ohne den alten Harness oder einen Menschen physisch am Schreiben zu hindern. Eine echte Beobachtung braucht Sessionbindung **und** Frische. Ein öffentlicher Listener kann erreichbar sein, ohne unbefugte Autorität zu verleihen. Umgekehrt hilft ein lokaler IPC-Pfad keinem entfernten Menschen ohne Zugang zum Gerät.

A6-Korrektur übernommen: `restore::reconcile` mit erhaltenen Leases ist nicht derselbe Pfad wie `reconnect_after_herdr_or_machine_restart` mit presumed-gone und Ersatzrecord. Der Ersatzrecord startet noch keinen Prozess. Ein Überlappungsschaden verlangt zusätzlich den tatsächlichen Ersatzstart und fortbestehende alte Schreibfähigkeit.

### 3. Zeit braucht einen Geschäftskontrakt

S021–S029 erhalten keine erfundene universelle Cron- oder monotonic-Regel. Ein gespeicherter Zeitauftrag erhält die **Bedeutung** einer Entscheidung; eine Occurrence-Zuordnung erhält die **bereits getroffene Aufnahmeentscheidung**. Keines ersetzt eine fehlende vertrauenswürdige aktuelle Uhr oder eine fremde Annahmequittung.

Zwei physische Termine in einer Foldminute können richtig sein. Eine ausdrücklich übersprungene Gapminute kann korrekt abgeschlossen verwaltet sein, ohne dass irgendeine Geschäftsarbeit ausgeführt wurde. Ein historischer Preis kann trotz abgelaufener Kaufgültigkeit ein korrektes Berichtsergebnis sein.

### 4. Die Außenwelt wird nicht durch lokale Records wahr

S046, S049, S051 und S081–S090 trennen:

- lokale beabsichtigte Operation;
- bloße Erfolgsmeldung;
- verbindlich belegte Teilnehmerannahme;
- menschlich geprüften Inhalt und erlaubte Terms;
- den operationsspezifischen Stand einer möglichen Inverse.

Ein lokaler Key zwingt keinen Zahlungsprovider zur Deduplizierung. Not-found nach Retentionsende beweist keine Nichtannahme. Ein Vorzustandsjournal führt keine Kompensation aus und enthält nicht automatisch spätere menschliche Änderungen. Eine gültige frühere Zustimmung autorisiert keinen geänderten Inhalt oder eine nach vertraglichem Ablauf angenommene Wirkung.

### 5. Erhaltene Aussage, Originalbytes und Wahrheit sind getrennt

S004, S016, S075, S076, S078, S080 und S098 liefern gegensätzliche Erhaltungsmuster:

- Taskrecord erhalten, Originaldatei weg;
- zuordenbare Originalarbeit vorhanden, Result-Commit fehlt;
- strukturell akzeptierte Ausgabe vorhanden, fachlich falsch;
- unabhängiger Prüfbezug vorhanden, fehlerhafte Ausgabe noch nicht akzeptiert.

Ein Digest über schon korrumpierte Bytes ist kein unabhängiger Wahrheitsbeleg. Auch ein zweiter zustimmender Mensch kann denselben Irrtum teilen. Der Prüfbezug muss die konkrete Eigenschaft messen und außerhalb des betroffenen Fehlerzusammenhangs liegen.

### 6. Rückkopplungen bleiben Hypothesen

S003, S024, S043, S060, S068 und S086 variieren eine Lastverstärkung: Rückstau erzeugt Timeouts oder scheinbar fehlenden Fortschritt, daraus entstehen zusätzliche Aufrufe/Tasks, diese senken nützliche Abschlussleistung und vergrößern den Rückstau. Bei S060 muss Retrylast die Drosselung tatsächlich verschärfen. Bei S068 braucht es erlaubte neue Wurzeln oder einen weiter erzeugenden Akteur; ein endlicher Delegationsbaum reicht nicht.

S092 hat eine andere Rückkopplung: Alarmdruck verschlechtert konkrete Freigabeprüfung, falsche Freigaben erzeugen Fehler, diese erzeugen neuen bindenden Dialogdruck. Ein einzelner falscher Klick, Stummschaltung, bewusste Vertagung oder eine endliche Notificationmenge belegt das nicht.

Prüfreihenfolge gemäß A6: Erst den ursprünglichen Fehler bei **festem Grundbedarf und gleicher Policy** entfernen und unterschiedliche Anfangslasten vergleichen. Danach getrennt Null-Neuzulauf testen. Drainage bei gleichzeitig reduziertem Bedarf widerlegt keine zwei Einzugsbereiche bei unverändert positivem Bedarf. Begrenztheit, wiederkehrendes Regime und tatsächliche Kanten müssen zusätzlich gezeigt werden. Ohne sie bleiben äußere Überlast, endliche Erholung oder Eskalation die ehrlichen Alternativen.

## Konkrete Splits und zulässige Wiederverwendung

Die IDs sind lokale Referenzen, keine beschlossene globale Modulaufteilung. Die freie ID OPR013 trägt keinen Kandidaten und wird nirgends referenziert.

| Gegenstände | Warum getrennt / wann wiederverwendbar |
| --- | --- |
| OPR001 Taskkern, OPR003 Snapshotpräfix, OPR004 kanonische Eingabebytes | Derselbe lesbare Taskkern wird in vielen Fehlerfällen wiederverwendet, aber ausschließlich als erhaltene Auftrags- und Annahmeinformation. Ein Snapshot hat eine Nachlaufgrenze; rohe Events einen Decoder-/Integritätsvertrag. Keiner dieser Gegenstände verspricht komplette historische Wahrheit. |
| OPR002 Zustellversuch, OPR015 Workspacelease, OPR029 Workerbelegung | Drei verschiedene Exklusionsfragen: noch erlaubte Zustellung, verwalteter Workspace-Neustart, neuer Task auf belegtem Worker. Unterschiedliche Stores, manuelle Prompts und lebende Nachkommen liegen nicht automatisch innerhalb derselben Grenze. |
| OPR005 Originalartefakt, OPR006 verwaiste Arbeitsbytes, OPR034 unabhängiger Prüfbezug | Originalinhalt, nachweisbare Runzuordnung ohne Completion und fachliche Wahrheit haben verschiedene Überlebensbedingungen. Einmal OPR005 wiederverwenden heißt immer dasselbe tatsächlich übernommene unveränderliche Objekt, nicht einen beliebigen Pfad. |
| OPR007 Ciphertext, OPR008 unabhängiger Export, OPR033 unabhängiger Wiederzugang | Verwahrbare verschlüsselte Bits sind noch kein lesbarer Inhalt. Ein Export muss wirklich lesbar außerhalb des Schadensbereichs existieren. Wiederzugang verlangt einen realen legitimen Abhängigkeitsweg, nicht eine Anleitung hinter derselben Sperre. |
| OPR010 ursprüngliche Queue, OPR011 Occurrence-Zuordnung, OPR012 Zeitauftrag | Nutzbarer begrenzter Arbeitsvorrat, eindeutige Terminaufnahme und beabsichtigte Zeitbedeutung sind verschiedene Verträge. Keine dieser Strukturen erneuert automatisch abgelaufene Arbeit. |
| OPR009 lokaler Bedienpfad, OPR017 Aktivierungsjournal, OPR018 Ressourcenreserve | Bedienbarkeit verlangt einen erreichbaren legitimen Nutzer. Diagnosebudget erhält einen Fehlerstand. Tatsächliche Hostreserve erhält Ressourcen unter Last. Subprozessaufsicht allein beweist die letzten beiden Eigenschaften nicht. |
| OPR014 korrelierte Beobachtung, OPR019 semantischer Operationsvertrag | Frische/Identität ist etwas anderes als gleiche Feldbedeutung. Autoritative alte Observation und erfolgreicher Majorhandshake können jeweils unzureichend sein. |
| OPR020 Intent, OPR021 Teilnehmerquittung, OPR022 Reviewobjekt, OPR023 Kompensationsstand | Beabsichtigt, tatsächlich angenommen, menschlich geprüft und konkret korrigierbar sind keine Synonyme. Eine Quittung dient auch der Kausalprüfung in S026, aber nur wenn sie genau die benötigte verbindliche Vorgängerversion enthält. |
| OPR024 Kaufabsicht, OPR028 Commandergebnis, OPR030 Delegationskette | Fachliche Gleichheit und gewünschte Menge, Retry desselben logischen Kommandos und Rückkehr in derselben Scopekette verlangen verschiedene Identitätsgrenzen. |
| OPR025 Principalbindung, OPR026 historische Scopeidentität, OPR027 Dispatchgeneration | Aufruferberechtigung, historische Zuordnung und Cross-Clone-Exklusivität sind unterschiedliche Autoritätsfragen. Ein Tombstone schließt keine Teilnehmerverpflichtung. |
| OPR016 objektgebundener Dateizugang, OPR031 literal gebundener Prozessauftrag | Pfad-/Symlinkkontinuität und argv-Interpretation sind verschiedene Grenzen. Ein literal übergebenes Argument kann fachlich weiterhin gefährlich sein. |
| OPR032 Entscheidungsdossier | Wiederverwendbar nur als tatsächlich verständlicher, zugänglicher Stand des konkreten Falls für einen kompetenten befugten Vertreter. Weder ein Blockerstring noch hundert quittierte Meldungen beweisen das. |

`module_hint` ist jeweils eine begrenzte fachliche Verantwortung, kein Auftrag für einen eigenen Dienst. Kernelzustand bleibt replaybar; Plugin-/Teilnehmersemantik und Runtimebeobachtung bleiben an expliziten Verträgen. Entwürfe für Artifactübernahme, Export und sichere Dateizugriffe müssen Factory-Schreiben innerhalb `.factory/` halten. Von Agenten geschaffene Taskdateien sind davon getrennt und dürfen nicht unter dem Vorwand der Analyse überschrieben werden.

## Die neun ursprünglich offenen Fälle

| Fall | Entscheidende fehlende Annahme | A7-Alternativen und Grenze |
| --- | --- | --- |
| S021 | Ist ein ziviles Label oder jeder physische Foldinstant ein Termin? | Ein Run für ein vereinbartes Label oder zwei Runs für zwei gewollte Instanten, jeweils mit dauerhafter Zuordnung. Ohne Vertrag bleibt die richtige Zahl offen. |
| S022 | Skip, nächster gültiger Zeitpunkt oder andere Geschäftspflicht? | Explizite Auslassung versus identifizierbare gültige Nachholung. Keine Gleichsetzung von ausgelassen und geschäftlich erledigt. |
| S023 | Clock-Domäne, Rücksprungerkennung und verfügbare vertrauenswürdige Zeit? | Erhaltene Terminidentität, Halt bei unbestimmbarer Expiry oder tatsächliche zeitliche Fehlannahme. Ein Cursor ist keine Ersatzwandzeit. |
| S025 | Gepinnte oder aktuelle tzdb und Cutover für bereits materialisierte Termine? | Alte Regeln erhalten oder legitim protokolliert neue Regeln anwenden. Ohne Absicht bleibt richtige Terminverschiebung offen. |
| S026 | Benötigt das Geschäft Reihenfolge und gibt es verbindliche Kausalbelege? | Belegte Vorgängerrelation, unabhängige gleichzeitige Effekte oder kein nutzbarer Beweis der nötigen Ordnung. Widersprüchliche Stempel allein reichen nicht. |
| S029 | Zählt Sleep zur verlangten Dauer, welches Primitive und welche Restartpersistenz? | Suspend-inklusive Dauer oder aktive Laufzeit können jeweils richtig sein. Gespeicherter Auftrag ohne rekonstruierbaren Zeitanker erhält die Absicht, nicht den aktuellen Ablaufwert. |
| S036 | Verbindliche Sessiongeneration/Sequenz am mutierenden Caller? | Alte Observation bleibt historisch, konkrete ungeprüfte Fehlmutation oder mangels Callerwissen offener Ausgang. C4 allein löst diese Frage nicht. |
| S048 | Welche Feldbedeutung und welche Invariante ändern sich tatsächlich? | Sichere Ignorierbarkeit, erkannter Vertragskonflikt, konkretes stipuliertes Beispiel echter statt simulierter Wirkung oder offen bei unbekanntem Feld. Das Simulationsfeld ist ausdrücklich kein behauptetes Factoryfeature. |
| S069 | Taskdisposition, Tombstone, erlaubter Historien-/Memoryzugang und Außenpflichten? | Lesbare erlaubte Historie bei lokaler Sperre, offene Teilnehmerpflichten, rechtmäßig gelöschte einzigartige Inhalte oder unbestimmter Stilllegungsvertrag. Keine heimliche Kopie als Ausweg aus Löschpflicht. |

Damit sind die fehlenden Annahmen konkreter, nicht automatisch entschieden. Besonders Zeitquellen, Quittungsbedeutung, Stilllegungsrechte und menschliches Verständnis benötigen vor Umsetzung explizite Vertragsentscheidungen.

## Gegenbeispiele und verbleibende Grenzen

- S006 unter dem echten Nur-zwei-verbrannten-Kopien-Inventar: kein benötigter Inhalt bleibt rekonstruierbar. Ein unabhängiger Export ist ein **anderer** ausdrücklich bedingter Zweig, keine spontane Rettungskopie.
- S017: verlorener bekannter Schlüssel lässt Recovery zunächst offen. Nachgewiesen verlorene sämtliche Entschlüsselungsinformationen innerhalb des benötigten Horizonts geben kein Klartextresidue, trotz intaktem Ciphertext.
- S016/S020/S070/S075/S080: ein Path oder fertige Berechnung rekonstruiert keine verlorenen einzigartigen Originalbytes.
- S090: fehlende Teilnehmerhistorie plus lokaler Intent beweist keine Nichtannahme. Ein unabhängig erhaltener echter Beleg ist eine eigene Voraussetzung, keine perfekte Prüfquelle.
- S085: der alte Vorwert einer Saga enthält nicht automatisch spätere menschliche Arbeit. Ohne deren erhaltene Version kann die Inverse sie dauerhaft zerstören.
- S037/S073: eine Storetransaktion schützt den betreffenden Pfad, nicht zwei Macs mit zwei kopierten Stores.
- S042/S047/S054: ein eigener Pluginprozess oder kleines Socketlimit erhält keine globale Reserve, wenn vorgelagerte Allokation oder andere Puffer unbeschränkt sind.
- S092: erhaltene Reviewobjekte beweisen keine verstandene Zustimmung. Eine endliche Alarmmeldung kann nicht für die Erhaltung aller offenen Vorfälle stehen.
- S100: gesperrte Anleitung, intakte lokale Bits oder laufende autonome Arbeit geben dem ausgesperrten Besitzer noch keinen nutzbaren Wiederzugang.

Bei Verlust und fehlender Information ist der Bezug stets die genannte Fähigkeit und der tatsächlich verfügbare, zulässige Bestand. Entdeckung einer weiteren Quelle widerlegt ein geschlossenes Inventar; sie darf nicht still vorausgesetzt werden. Gegenwärtig fehlender Zugang ist schwächer als endgültige Vernichtung.

## Prüfbedarf vor einer Umsetzung oder stärkeren Behauptung

Alle folgenden Nachweise sind **vorgeschlagen, nicht ausgeführt**. Nur synthetische isolierte Fixtures mit Fake-Teilnehmern, keine Livefehler oder echten Sendungen.

| Vertragsgrenze | Unterscheidender Test |
| --- | --- |
| Commit, Cancel, Restore | Writer-Schnittpunkte instrumentieren, Caller-Serialisierung einschließen; Snapshot vor extern bekanntem späterem Effekt öffnen. Sichtbarer Nachlauf muss von unbekanntem getrennt bleiben. |
| Artifact und Prüfung | Bequemen Pfad nach echter Objektübernahme ändern/löschen; Worker nach Flush vor Completion stoppen; falsche Runzuordnung und semantisch falsche aber gültig geformte Ausgabe einspeisen. |
| Identität und Dateizugang | Neue Pane mit alter Adresse, fehlende UUID und verspätete Generation kombinieren; Casealiases in einem Store und zwei Clones getrennt prüfen; Symlink genau zwischen Prüfung und Zugriff tauschen. |
| Zeit | Fold, Gap, zwölf Stunden Rücksprung, Sleep, Restart und zwei tzdb-Versionen mit zuvor festgelegter Absicht testen. Richtige Anzahl allein genügt nicht ohne richtige Geschäftssemantik. |
| Pluginaufnahme | Debugtext, Riesenlänge vor Body, FD-Sturm, nichtlesenden Client und Hostrestart während Crashbudget testen. Konkrete lokale Arbeit und gesamten Ressourcenverbrauch beobachten. |
| Semantik und Teilnehmer | Altes/neues optionales Feld mit gegensätzlicher Wirkung, Erfolg vor Commit, ignorierten Key, verlorene Antwort und abgelaufenen Lookup nachstellen. Reale Fake-Effektzahl getrennt vom lokalen Wissen zählen. |
| Freigabe und Inverse | Inhalt, Instanz, Preis und Annahmezeit nach Klick verändern; menschliche Zwischenversion vor Inverse setzen. Kein späterer Erfolg darf frühere Zustimmung oder verlustfreie Kompensation fingieren. |
| Last und menschliche Rückkopplung | Fehlerende bei festem Grundbedarf und gleicher Policy zuerst, Nullzulauf danach. Bei Alarmhypothese beide tatsächlichen kausalen Kanten prüfen statt feste Fehlerwahrscheinlichkeit als Menschennachweis einsetzen. |
| Zugang und Stilllegung | Synthetische Recoveryabhängigkeiten ohne verlorenes Gerät durchgehen; kompetenten befugten Vertreter nur mit vorhandenem Dossier entscheiden lassen; alte Tasks/Memory/Teilnehmerpflichten nach Stilllegung separat prüfen. |

## Quellen und übernommene A6-Auflagen

Vollständig gelesen: `../A7-PROTOCOL.md`, `../REVIEW-PROTOCOL.md`, eigene `scenarios.csv`, `states.csv`, `trajectories.csv` in beiden vollständigen Teilen, `coverage.csv`, ursprünglicher `report.md`, ferner `../../docs/residuality/a6/state-qualifications.csv`, `supplemental-branches.csv` und `../skeptic/cross-review.md`.

Die A6-Qualifikationen sind übernommen, insbesondere getrennte Beobachtung/Ausführung (QF01), einzelne Wartebedingungen und Quellpfade (QF03–QF05), konkrete Guardgrenzen und begrenzte Abschlüsse (QF06–QF07), feste Vergleichslast und echte Erzeuger (QF09–QF11), geschlossene Informationsinventare (QF12), Wirkung versus Schaden (QF13), Bytes/Bedeutung/Completion (QF14), Stilllegung versus Außenpflicht (QF16) und source-lokale Aussagen statt implementierter Gesamtsaga (QF17). Die Supplemental-Datei enthält keine neue S001–S100-Karte; ihre nichtschleifenden Alternativen und das Verbot erfundener Schutzgrenzen wurden methodisch berücksichtigt, ohne fremde Fälle zu übernehmen.

C1–C12 und D1–D3 in den Fällen beziehen sich auf den unveränderten Quellenregister des ursprünglichen `report.md`. A7 hat diese Produktquellen **nicht erneut als aktuellen Implementierungsstand geprüft**. Die damaligen schmalen Befunde, etwa `deliver`, `complete_with_result`, `assign`, `begin_start` und Delegationsprüfung, sind keine Belege für einen heute laufenden vollständigen Eventstore, Pluginhost oder Saga-Runner. Die Architekturentwürfe bleiben davon getrennt.

## Selbstprüfung und eingefrorene Eingaben

Mechanische Prüfung der endgültigen Daten:

- genau S001–S100, einmal und in Reihenfolge; keine fremden Stressoren;
- genau die vorgeschriebenen JSON- und CSV-Felder, Semikolon-CSV und UTF-8;
- alle Zweige mit nichtleeren Bedingungen, Persistenz, Grund, Nachweis und Szenariokonsequenz;
- nur erlaubte Zustandsarten und Residue-Statuswerte;
- ausschließlich existierende historische OP-IDs, Herkunft exakt zur alten Coverage;
- alle referenzierten Kandidaten existieren, jeder Kandidat wird benutzt, keine doppelten Referenzen im Zweig;
- bei `bedingt` und `teilweise` mindestens ein passender Kandidat; leere Listen nur bei ausdrücklich begründeter fehlender Fähigkeit oder ungeklärter Nutzbarkeit;
- 273 lokale Zweige und 33 lokale Kandidateneinträge als reine Dateistatistik, keine globale Zielzahl und kein Vollständigkeits-/Konvergenznachweis;
- historische und A6-Eingaben gegenüber vor Beginn gemessenen SHA-256 unverändert;
- nur `a7/residues.csv`, `a7/cases.json`, `a7/report.md` als neue Arbeitsdateien.

Beim inhaltlichen Abschlussreview wurden S027 um die konkret zu spät angenommene Wirkung, S036 um eine ausdrücklich vorausgesetzte tatsächliche Caller-Fehlmutation und S048 um das konkrete hypothetische Semantik-Gegenbeispiel ergänzt. Kein erfolgreicher Schutzpfad verdrängt damit diese Schadensalternativen.

Die folgende reine Leseprüfung ist aus dem eigenen Reviewverzeichnis reproduzierbar. Sie prüft Artefaktintegrität, nicht sachliche Wahrheit oder Attraktion:

```python
import collections, csv, hashlib, json, pathlib, re
root = pathlib.Path('.')

def rows(path):
    with (root / path).open(encoding='utf-8', newline='') as f:
        return list(csv.DictReader(f, delimiter=';'))

cases = json.loads((root / 'a7/cases.json').read_text(encoding='utf-8'))
residues = rows('a7/residues.csv')
coverage = rows('coverage.csv')
scenarios = rows('scenarios.csv')
trajectories = rows('trajectories.csv')
historical = {r['state_id'] for r in rows('states.csv')}
expected = [f'S{i:03d}' for i in range(1, 101)]
assert [c['stressor_id'] for c in cases] == expected
assert [r['stressor_id'] for r in scenarios] == expected
assert [r['stressors'] for r in trajectories] == expected
assert [r['stressor_id'] for r in coverage] == expected
rfields = 'id name object survival prerequisites failure_boundary module_hint architectural_change validation'.split()
assert list(residues[0]) == rfields
ids = {r['id'] for r in residues}
assert len(ids) == len(residues) == 33
for r in residues:
    assert list(r) == rfields
    assert re.fullmatch(r'OPR\d{3}', r['id'])
    assert all(isinstance(v, str) and v.strip() for v in r.values())
used, kinds, statuses = collections.Counter(), collections.Counter(), collections.Counter()
allowed = set('attraktorhypothese transient extern-erzwungen halt ungewissheit abschluss verlust eskalation offen'.split())
for case, old, trajectory in zip(cases, coverage, trajectories):
    assert set(case) == set('stressor_id source_states branches architectural_consequence'.split())
    sources = [] if old['state_ids'] == 'none' else old['state_ids'].split()
    assert case['source_states'] == sources
    assert trajectory['destination_states'] == old['state_ids']
    assert set(sources) <= historical
    assert case['architectural_consequence'].strip() and case['branches']
    for branch in case['branches']:
        assert set(branch) == set('state kind conditions persistence residue_ids residue_status reason validation'.split())
        assert all(v.strip() for v in branch.values() if isinstance(v, str))
        assert branch['kind'] in allowed
        assert branch['residue_status'] in {'bedingt', 'teilweise', 'keines', 'ungeklaert'}
        refs = branch['residue_ids']
        assert isinstance(refs, list) and len(refs) == len(set(refs))
        assert set(refs) <= ids
        if branch['residue_status'] in {'bedingt', 'teilweise'}:
            assert refs
        used.update(refs)
        kinds[branch['kind']] += 1
        statuses[branch['residue_status']] += 1
assert set(used) == ids
assert sum(kinds.values()) == 273
assert {p.name for p in (root / 'a7').iterdir()} == {'cases.json', 'residues.csv', 'report.md'}
report = (root / 'a7/report.md').read_text(encoding='utf-8')
section = report.rsplit('\n### Eingabe-SHA-256,', 1)[1]
checks = re.findall(r'\| [^\n]*?`([^`]+)` \| `([0-9a-f]{64})` \|', section)
assert len(checks) == 8
for name, digest in checks:
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
print('PASS: 100 Fälle, 273 Zweige, 33 benutzte lokale Kandidaten, 8 unveränderte Eingaben')
print('Zustandsarten:', dict(kinds))
print('Residue-Status:', dict(statuses))
```

### Eingabe-SHA-256, zu Beginn gemessen und am Ende verglichen

| Datei | SHA-256 |
| --- | --- |
| `scenarios.csv` | `6d0b9a793a017a1bda18cf689c55293df6fee926a443f51bfc8b24f8abf1477d` |
| `states.csv` | `0d7b2b601bc591a722afbed718e2fb7bf3b7bed3d7fa1dd2bb31073fcdce3cc7` |
| `trajectories.csv` | `3a4a7401331fa477468ba2cd81b5912996459dda35763174be856d6d9a8003e0` |
| `coverage.csv` | `c36d39bae59fa5a257b867bffb5a4528ef679882f3fd7d56960b3cb5eea8c2eb` |
| ursprünglicher `report.md` | `b7b060812949528cfece0c0160593fd696ca28ff70100e0e9e91bf98cfd3bd20` |
| `../../docs/residuality/a6/state-qualifications.csv` | `0c699699d3fb6d914403b20fa496295ba73a0662762c8317b59b83f08d4d8973` |
| `../../docs/residuality/a6/supplemental-branches.csv` | `2b7d96ba89f70d6ef950481be8e9dd1959ad4bdf741a003bd37ae51096ada7a4` |
| `../skeptic/cross-review.md` | `5d0a9b0c8e531fb6f0cd3fc3243648d5f94c857d8e51b2edefc1cda3b846e65b` |

Die Hashes zeigen Unverändertheit dieser gelesenen Eingaben, nicht ursprüngliche Autorschaftsunabhängigkeit oder beobachtetes Produktionsverhalten. Entscheidungen, Fortschritt und Abgabe werden gegen den zugewiesenen Run dokumentiert.
