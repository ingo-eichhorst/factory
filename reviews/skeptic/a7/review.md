# A7-Gegenreview: vollständig erfasst, noch nicht überall passend zugeordnet

Run: `c5932c99-b970-464b-a22c-6344a3b4f468`  
Stand: 2026-09-09. Referenzen beziehen sich auf den geprüften A7-Freeze; Projektpfade sind relativ zum Factory-Projektroot.

## Urteil

**A7 ist als vollständiges bedingtes Fallinventar brauchbar. Die Aussage, alle Zuordnungen und Modulverträge seien bereits inhaltlich geschlossen, trägt es noch nicht.**

Ich habe alle vier abgeschlossenen Berichte, alle vier Residue-Kataloge und sämtliche Felder aller 350 Quellfälle mit ihren 852 Zweigen gelesen. Die sieben Felder jeder Zeile von `generated/coverage.csv` stimmen mit der Ableitung aus diesen Quellen überein. Alle zwölf eingefrorenen A7-Abgaben sind unverändert. Es fehlen keine Stressor-IDs oder Referenzen.

Das ist mehr als eine leere Matrix: Die Fälle enthalten überwiegend konkrete Voraussetzungen, eine Erklärung des Fortgangs, eine begrenzte Restfähigkeit und eine Gegenprüfung. Aber ein nichtleeres Begründungsfeld ist noch keine richtige Begründung. Besonders **S156.B01 zählt ein korrektes Publikationspaket, das unter seinen Bedingungen noch gar nicht vorhanden sein muss**. Dazu kommen unpassende Wiederverwendungen und Lücken beim Übertragen unabhängiger Schutzgrenzen in die Modultabellen.

`findings.csv` enthält neun konkrete Auflagen: **eine hohe, sieben mittlere und eine niedrige**. „Hoch“ bedeutet hier eine vor positiver Zuordnungsfreigabe zu korrigierende Aussage, nicht eine festgestellte Sicherheitslücke laufender Software. Es wurden weder Produktverhalten noch reale Teilnehmer getestet.

Die 152 Kandidaten sind herkunftserhaltende Entwurfsverträge. Die 16 Module sind Verantwortungen. Auch die 65 als `attraktorhypothese` bezeichneten Zweige sind **keine Zahl verschiedener oder bewiesener Attraktoren**. Ich schlage keine Zielzahl und keinen automatischen Residue-Merge vor.

## 1. Konkrete Fehler in der Zuordnung

### A7S01 — Ein erst herzustellendes sauberes Paket überlebt noch nicht

**S156.B01 → GVR036 → M07** ist der deutlichste Fehler.

Der Zweig sagt: „Falsches lokales Paket liegt vor“. Die Übertragung ist noch nicht angenommen, die Prüfung findet Company-Inhalte. Laut Fortgang bleibt die Freigabe gesperrt, **bis** korrekt projizierte Bytes geprüft sind.

GVR036 ist aber bereits ein „erzeugter und inhaltlich geprüfter Projekt-Objektgraph ohne ausgeschlossene Company-Bereiche“. Das ist ein anderer Gegenstand. Ein Prüfer kann das falsche Paket erkennen und zurückhalten, ohne bereits ein richtiges Paket zu besitzen. Der positive Status `bedingt` nennt hier das gewünschte spätere Arbeitsergebnis als vorhandenen Rest.

**Korrektur:** Im gehaltenen Zweig nur vorhandene und rechtmäßig nutzbare Fehlpaket-/Prüfevidenz oder eine tatsächlich gespeicherte gesperrte Publikationsabsicht zuordnen. GVR036 braucht einen eigenen Zweig mit bereits vorhandenem geprüften Projektpaket. M07 erhält dessen Bytes; M02 verantwortet die getrennte Sendefreigabe; M15 den zulässigen Datenumfang. Keine dieser Rollen soll allein durch ein korrektes Paket eine Veröffentlichung erlauben.

Der unterscheidende Gegenfall ist einfach: Das Rootpaket existiert, der Filter lehnt es korrekt ab, ein sauberes Projektpaket wurde noch nie erzeugt. Die Schutzreaktion funktioniert; **GVR036 existiert trotzdem nicht**.

### A7S02 — Noch kein Versuch ist nicht ein erhaltener früherer Versuch

**S074.B02 → OPR002 → M06** setzt ausdrücklich voraus, dass noch kein Versuch verbraucht wurde. OPR002 bezeichnet dagegen eine gespeicherte Versuch-ID mit verbrauchter Autorisierung; sein Nutzer untersucht den früheren Versuch.

Bei **S074.B03** kommt eine weitere Autorisierungsrevision hinzu. Auch diese darf nicht still mit dem unverändert verbrauchten Budget gleichgesetzt werden.

**Korrektur:** Unverbrauchte Zustellautorisierung, tatsächliche Versuchsevidenz und spätere neue Autorisierung getrennt benennen. Sie dürfen dieselbe technische Speicherung benutzen. Aber B02 darf keinen früheren Versuch behaupten, und ein neuer Grant klärt weder alten Empfang noch alte Außenwirkung.

### A7S03 — Ein WebSocket-Cursor ist keine Runtimebeobachtung

**S054.B01/B02 → OPR014** begründen Clientcursor, Frischeanzeige und erhaltenen Ereignissuffix. OPR014 verlangt dagegen eine echte Runtime-Sessiongeneration, Beobachtungssequenz und Prüfung vor Lifecycle-Mutation.

Ein Client kann nach einer Transportpause aus einem Ereignisfenster nachlesen, ohne eine einzige Runtimeobservation zu besitzen. Der Verweis trifft dann den falschen Gegenstand.

**Korrektur:** M01 verantwortet Cursor und erhaltenen Ereignisbereich, M05 den begrenzten Transport. Runtimeidentität und ihre Frische bleiben ein eigener Vertrag. OBR029 zeigt einen passenden Vergleichsvertrag für Subscriber; daraus folgt **kein automatisches Zusammenlegen** mit OPR014 oder anderen Beobachterkandidaten.

## 2. Nicht jede Rückkopplung trägt die gewählte Zustandsart

**A7S04: S130.B02 und S196.B02** nennen fortgesetzte Erzeugung neuer Wurzeln oberhalb des Abbaus ohne Gesamtlimit. Sie geben aber keine Sättigung oder begrenzte Wiederkehr an. Solange Erzeugung größer als Abbau bleibt, wächst die Menge. Ressourcenende unterbricht diese Dynamik; es beweist keinen anziehenden wiederkehrenden Zustand.

Das ist zunächst **Eskalation oder offene Dynamik**, nicht die dort gewählte Attraktorhypothese. Ein weiterer Hypothesenzweig wäre möglich, wenn Begrenzung, Rückkehrmechanismus und Eintrittsbedingungen konkret hinzukommen. Die Operations-Abgabe formuliert diese Zusatzbedingung beispielsweise in S003.B03 deutlich sauberer.

Umgekehrt begründen **S235.B02 und S245.B02** nur zwei gleichzeitig schreibfähige Prozesse. Das kann ein anhaltender Konflikt sein. Ohne zusätzliche Erzeugung oder wachsenden Schaden ist `eskalation` nicht hergeleitet.

Diese Korrektur verlangt keine neue Residuezahl. GVR001 und OBR001 können als lesbare frühere Akten weiter passen, sofern ihr Zugriff tatsächlich erhalten bleibt. Sie beweisen weder Wachstum noch dessen Begrenzung.

## 3. Die Modulidee ist gut; die Grenzverträge sind noch zu grob

Positiv ist: ein mutierender Daemon, reine Wiedergabe in M01, Adapter und Teilnehmer hinter begrenzten Verträgen, ausdrückliche Trennung von lokalem Journal und fremder Annahme. M13 soll bestehende Tasks verwenden, **keinen v1-Message-Store**. M12 bekommt ausdrücklich kein Reparatur- oder Geschäftsrecht. Eine gemeinsame Bibliothek wird nicht als gemeinsame Strom- oder Angreifergrenze ausgegeben.

Die primären Zuordnungen sind explizit. `bind_modules.py` legt jedoch die meisten Mitwirkungsmodule und Begründungen anhand des primären Moduls fest; nur einzelne Kandidaten haben Ausnahmen. Das ist kein automatischer Residue-Merge. Es reicht aber nicht überall für die unterschiedlichen Ausfallverträge. Entscheidend sind drei konkrete Stellen:

### A7S05 — BDR037 braucht einen anderen Akteur als den Beobachter

**S350.B01/B04 → BDR037 → M12** enthalten nicht nur unabhängige Vergleichsevidenz. BDR037 braucht auch einen legitimen Akteur, der dem kompromittierten Code tatsächlich Schreib- und Prüfmacht entziehen kann.

M12 darf genau das nicht selbst tun. Die Bindung mit M10/M14/M15 erklärt die Evidenzgrenze, legt aber nicht fest, welcher getrennte Akteur den Entzug bei kompromittiertem Kernel vollzieht und welche OS- oder Teilnehmergrenze er erreicht.

**Korrektur:** Vergleich, legitime Eingriffsentscheidung und wirksamen Entzug innerhalb dieses Kandidaten ausdrücklich getrennt zuordnen. Insbesondere erklären, was bei unerreichbarem Entscheider oder fehlendem Entzugspfad übrig bleibt: möglicherweise nur ein brauchbarer Gegenbeleg, nicht der volle Eingriffsvertrag. Dem Zeugen dafür keine Shell geben und keinen zweiten schreibenden Kernel bauen.

Das ist eine Lücke der Zuordnung, **kein Beleg, dass die Zielarchitektur bereits einen unzulässigen Zweitkernel vorsieht**. Ihr gegenteiliger Grundsatz ist richtig und muss erhalten bleiben.

### A7S06 — Autonomer Notstopp darf nicht durch die normale Factory-Aufrufkette müssen

**S176.B01, S321.B01, S322.B02 → GVR035/BDR026/BDR027 → M16** setzen einen unabhängigen lokalen Controller voraus. Die Bindung nennt dafür dieselbe Mitwirkung M02/M06/M10/M14 wie für gewöhnliche Geschäftsadapter. Die `command`-Kante M16→M14 und weiter M14→M02 grenzt ihren Anwendungsbereich nicht auf vorgelagerte Factory-Aufträge ein.

Wenn jede lokale Sicherheitsreaktion diese Kette bräuchte, wäre sie gerade nicht unabhängig vom ausgefallenen Factory-Kern. Wenn nur der vorgelagerte Factory-Aufruf gemeint ist, muss die Tabelle das sagen.

**Korrektur:** Für jede relevante Operation festlegen, ob eine Mitwirkung zwingend, nur vorgelagert oder erst zur späteren Belegübernahme nötig ist. Lokale Energie, Sensoren, Sicherheitsentscheidung und Aktuatorgewalt brauchen einen eigenen Betriebsvertrag außerhalb der laufenden Factory-Instanz. Das heißt ausdrücklich **nicht**, Factory um einen Sicherheitscontroller zu erweitern. Der Fließtext in §8 nennt die richtige Grenze bereits.

### A7S07 — Ein alter Faktenkern kann seinen späteren Widerruf nicht selbst liefern

**S127.B01/S191.B01 → GVR011 → M02 mit M01/M14** verlangen einen nicht zurückgesetzten aktuellen Widerrufsstand. M02 besitzt Grants und Widerrufe jedoch als Kernstand; die Zuordnung benennt noch keine konkrete unabhängige dauerhafte Quelle mit Besitzer und Zugriffspfad.

Die Zielarchitektur fordert den frischen Abgleich bereits richtig. Offen bleibt, wer diesen stärkeren Vertrag unter dem angenommenen Restore trägt. Ein M16-Receipt belegt einen Ausgang, eine Schreibgeneration einen Writerbereich. Keines ist automatisch ein späterer personenbezogener Widerruf.

**Korrektur:** Maßgebliche unabhängige Autoritätsquelle und tatsächliche Annahmeprüfung zuordnen. Ist diese Quelle nicht vorhanden oder nicht frisch erreichbar, bleibt die engere Sperre neuer Arbeit. Das ist nicht dasselbe wie ein erhaltenes, aktuell nutzbares Widerrufsregister. Testfall: alter Snapshot, späterer Widerruf, anschließend Ausfall der Gegenquelle.

Diese drei Punkte verlangen präzisere Verträge, nicht mehr Module oder eine universelle Pipeline.

## 4. Freeze und Prüfkommando brauchen eine klare Grenze

**A7S08:** `build.py` erzeugt `Sxxx.Bnn` aus der Listenposition. Eine rein im Speicher vorgenommene Umordnung der beiden S156-Zweige wird akzeptiert; danach bezeichnet S156.B01 die tatsächliche Offenlegung statt des gehaltenen Rootexports. `load_analysis/render` erzwingen den A7-Freeze nicht.

Die Originale sind aktuell unverändert. Das Problem betrifft die dauerhafte Bedeutung der jetzt entstehenden Gegenreviewreferenzen. Korrekturen sollten daher als **Nachtrag mit Ursprungshash und Branch-ID** geführt werden, nicht durch Umsortieren der eingefrorenen Entdeckung. Spätere Datenversionen brauchen stabile IDs oder ausdrücklich gebundene Revisionen.

**A7S09:** `build.py --check` lädt `module-dependencies.csv` nicht. Ein im Speicher angebotener M07→M01-Bootstrapzyklus lässt seine Ausgabe unverändert. Der separate vorhandene Dependencytest erkennt diesen Zyklus dagegen korrekt. Ich beanstande deshalb nicht das Fehlen jeglicher Zyklusprüfung, sondern den unterschiedlichen Umfang der Prüfwege und die zu allgemeine Compiler-Erfolgsmeldung.

Ein vollständiger Read-only-Prüfaufruf sollte Build, Bindings, Abhängigkeiten und A7-Freeze gemeinsam prüfen. Auch dieser Aufruf beweist nur Dokumentkonsistenz.

## 5. Harte Grenzen, die A7 bereits ehrlich behandelt

Diese Befunde sollten bei Korrekturen nicht verloren gehen:

- **Totalverlust:** S006.B01, S190.B01 und S345.B01 erhalten kein erfundenes Backup. S006.B02 ist ausdrücklich ein anderer Zweig mit vorher vorhandener unabhängiger Kopie.
- **Keine Schlüssel:** S017.B03 und S271.B02 behaupten keinen Klartextrest. OPR007 ist nur verwahrbarer Ciphertext, kein rekonstruierter Taskbestand.
- **Alarmwege und Menschen fehlen:** S206.B01 bewahrt nur später lesbaren Bestand; S207.B01 und S208.B01 behaupten weder aktuelle Fernevidenz noch menschlichen Empfang. Die Ersatzempfängerzweige ändern ihre Voraussetzungen ausdrücklich.
- **Keine Prüfreferenz:** S184.B03, S346.B01 und S350.B02 erfinden kein internes Wahrheitsorakel. Konsistenz ist dort keine Weltkenntnis.
- **Rechtskonflikt:** S186.B01, S319.B03 und S344.B03 lösen wörtlich widersprüchliche Pflichten nicht durch Hashes oder heimliche Kopien. S194.B02 erhält keine gerichtlich gelöschte notwendige Payload.
- **Teilnehmer statt Journal:** S081.B01/OPR020 bleibt bei ignoriertem Idempotenzschlüssel unbekannt. S090.B02/OPR021 setzt einen wirklich erhaltenen Receipt voraus. S170.B02/GVR021 bezeichnet ausdrücklich nur historische Einsicht, nicht weiter wirksames Fencing. S256.B02 verlangt für endgültige Nichtannahme zusätzlich das Ende weiterer Annahmemöglichkeit.
- **Beobachter und Welt:** S326.B01 lässt das Geschäft trotz Monitorverlust weiterlaufen. S330.B01–B03 unterscheiden Aussagen, Deutung und tatsächliche Wirkung. Ein falscher Glaube wird nicht automatisch zum falschen Werkprodukt.

## Nächster Schritt

Die eingefrorenen Quellen nicht ändern. Zuerst die neun Auflagen in einem zugeordneten Korrekturenstand beantworten, besonders S156.B01. Danach die operations- und ausfallbezogenen Modulverträge präzisieren und die Referenzen erneut prüfen. Erst ein getrennt beauftragter Implementierungs-/Vertragsversuch kann zeigen, ob diese Fähigkeiten wirklich bestehen.

Es gab hier keine Implementierung, kein Livefehlerexperiment, keine Sendung, keine Kontoveränderung und keine Freigabe solcher Aktionen. Die ausgeführten Lese- und Strukturprüfungen sowie ihre Grenzen stehen in `validation.md`.
