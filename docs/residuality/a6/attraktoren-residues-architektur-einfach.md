# Von den Attraktor-Zuständen zur Architektur — einfach erklärt

**Erweitert durch A7:** Dieses Dokument erklärt die damalige Teilmenge von 19 Attraktorhypothesen. Die [neue vollständige Herleitung](../a7/generated/stressoren.md) behandelt alle 350 Stressoren samt weiteren Zustandsarten und Residue-Dispositionen. Dazu gehören der [vollständige Residue-Katalog](../a7/generated/residues.md) und die [überarbeitete modulare Zielarchitektur](../a7/zielarchitektur.md). Die ursprünglichen 19 Abschnitte bleiben hier nachvollziehbar erhalten.

## Worum es hier geht

Dieses Dokument erklärt **jeden der 19 Zustände, die die unabhängigen A6-Analysten ausdrücklich als Attraktorhypothese bezeichnet haben**. Jeder bekommt einen eigenen Abschnitt:

> **Welche Stressoren können dorthin führen? → Warum bleibt das Problem bestehen? → Was kann noch brauchbar bleiben? → Was müsste Factory dafür anders bauen?**

Ein **Stressor** ist eine Störung oder Belastung. Ein **Attraktor** ist vereinfacht ein Zustand oder wiederkehrender Ablauf, zu dem das System unter bestimmten Bedingungen hingezogen wird. Hier untersuchen wir insbesondere Probleme, die sich selbst verstärken.

Ein **Residue** ist eine Struktur oder Fähigkeit, die unter den betrachteten Störungen noch brauchbar bleibt. Das ist **nicht einfach der Fehlerzustand** und auch nicht automatisch die Maßnahme, die wir uns dagegen wünschen.

**Wichtig:** Die 19 Zustände sind begründete Vermutungen, keine nachgewiesenen Factory-Attraktoren. Ebenso sind die hier hergeleiteten Residues **Vorschläge mit Voraussetzungen**, keine bewiesenen Eigenschaften der heutigen Software. Die Architekturänderungen beschreiben, was wir dafür bauen, absichern oder genauer festlegen müssten. Manche konkretisieren bereits vorhandene Architekturentscheidungen; sie sind nicht alle neu und hier nicht beschlossen oder implementiert.

### So sind die Abschnitte zu lesen

- **Stressoren:** alle Szenarien, deren A6-Verlauf diesen Zustand als möglichen Ausgang nennt. Ihre Nummern erlauben den Rückweg zur ursprünglichen Analyse. Sie führen **nicht zwangsläufig** dorthin.
- **Verlauf:** die zusätzliche Rückkopplung, ohne die es diesen Attraktor nicht gibt.
- **Was bleibt / Residue:** erst die möglicherweise vorhandenen Reste, dann die daraus abgeleitete brauchbare Restfähigkeit und ihre Voraussetzung.
- **Architekturänderungen:** konkrete Vorschläge, um diese Restfähigkeit zu erhalten oder nutzbar zu machen.
- **Grenze und Prüffrage:** wann die Herleitung nicht gilt und was noch nachgewiesen werden müsste.

Ein *Projektbereich* heißt in Factory auch *Scope*. *Plugins* sind austauschbare Erweiterungen, etwa die Anbindung eines externen Dienstes. Der *Kern* ist die zentrale Instanz, die Factory-Zustand verändert. Ein *Teilnehmer* ist beispielsweise ein externer Zahlungs- oder Versanddienst, der eine tatsächliche Handlung annimmt.

## Übersicht

| Zustand | Einfacher Name |
|---|---|
| [OP004](#op004) | Aus Arbeit entsteht immer mehr Arbeit |
| [OP032](#op032) | Alarmdruck führt zu schlechten Freigaben |
| [GV05](#gv05) | Ein Irrtum bestätigt sich selbst |
| [GV06](#gv06) | Gute Testnoten verdrängen echte Prüfung |
| [GV11](#gv11) | Aufträge vermehren sich schneller, als sie fertig werden |
| [GV16](#gv16) | Regelbruch wird zum Erfolgsrezept |
| [GV23](#gv23) | Der stärkste Projektbereich bekommt immer mehr Kapazität |
| [GV29](#gv29) | Mehr Kerndienste erzeugen Bedarf nach noch mehr Kerndiensten |
| [OB11](#ob11) | Wiederholungsversuche halten die Überlast am Leben |
| [OB22](#ob22) | Die Wiederherstellung installiert den Angreifer erneut |
| [OB34](#ob34) | Fehlalarme erziehen Menschen zum Wegsehen |
| [BD05](#bd05) | Falsche Daten werden zur eigenen Wahrheitsquelle |
| [BD08](#bd08) | Zwei schreibende Instanzen reparieren gegeneinander |
| [BD13](#bd13) | Reparaturen machen die nächste Reparatur schwerer |
| [BD14](#bd14) | Ausnahmen machen Berechtigungen immer unklarer |
| [BD18](#bd18) | Meldungen verbrauchen die Zeit für die Reparatur |
| [BD24](#bd24) | Zusätzliche Kopien machen langsame Antworten noch langsamer |
| [BD28](#bd28) | Die Kennzahl wird besser, die Leistung nicht |
| [BD35](#bd35) | Schadsoftware bescheinigt sich selbst, dass alles gut ist |

Ähnliche Zustände bleiben getrennt, wenn ihre Ursachen oder Auswege verschieden sind. Deshalb ergeben 19 Abschnitte **weder 19 voneinander unabhängige Attraktoren noch 19 verschiedene Residues oder neue Dienste**.

Die Abschnitte enthalten **89 Zuordnungen zu 84 verschiedenen Stressoren**. Die übrigen Szenarien der 350er-Analyse sind diesen ausdrücklich markierten Hypothesen nicht zugeordnet. Sie betreffen etwa Wartezustände, erzwungene Ausfälle oder Verlust, oder ihre Dynamik blieb ungeklärt. Das bedeutet nicht, dass dort grundsätzlich keine weiteren Attraktoren möglich wären.

<a id="op004"></a>
## 1. OP004 — Aus Arbeit entsteht immer mehr Arbeit

### Stressoren

- **S003:** Der Rechner wird heiß und langsamer; bisherige Zeitschätzungen stimmen nicht mehr.
- **S024:** Nach sechs Wochen Schlaf sind Tausende geplante Aufträge fällig.
- **S043:** Ein Plugin beantwortet Zustandsprüfungen, aber keine Geschäftsaufträge.
- **S060:** Ein externer Dienst begrenzt Anfragen über mehrere Tage.
- **S068:** Ein Agent legt immer neue, inhaltlich gleiche Aufträge an.
- **S086:** Eine ausgleichende Handlung scheitert wiederholt.

### Wie entsteht der Zustand?

Arbeit dauert länger → Factory oder ein Agent startet Ersatzversuche → diese verbrauchen zusätzliche Kapazität → noch mehr Arbeit überschreitet ihre Frist → weitere Ersatzversuche entstehen.

Die ursprüngliche Hitze oder Störung kann längst vorbei sein. Das System erzeugt seine Belastung inzwischen selbst.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** der ursprüngliche Auftrag mit einigen bereits fertigen Ergebnissen, sofern Speicher und Zugriff noch funktionieren.

**Brauchbares Residue:** ein begrenzter Bestand eindeutig zugeordneter Aufträge, den Factory noch ansehen und kontrolliert abarbeiten kann, ohne ihn dabei zu vervielfachen. Dafür müssen Auftrag, Versuche und Ergebnisse unterscheidbar erhalten bleiben.

### Welche Architekturänderungen folgen daraus?

1. Neue Aufträge, Wiederholungen und Ausgleichsversuche dürfen nicht unabhängig voneinander unbegrenzt Kapazität belegen. Sie brauchen eine gemeinsame Zulassungsgrenze.
2. Ein erneuter Versuch bleibt mit dem ursprünglichen Auftrag verbunden. Ein neuer Name darf nicht unbemerkt das Versuchslimit zurücksetzen.
3. Ein Betriebsmodus nimmt keine zusätzliche Arbeit an und arbeitet nur noch den zulässigen Bestand ab. Diagnose und Stoppen müssen auch bei Überlast erreichbar bleiben.
4. Alte Aufträge werden vor Ausführung auf Frist und aktuelle Freigabe geprüft. „Noch in der Warteschlange“ bedeutet nicht „noch erlaubt“.

**Grenze und Prüffrage:** Eine große, aber endliche Welle ist noch kein Attraktor. Unbegrenztes Wachstum bis zum Zusammenbruch ist ebenfalls kein nachgewiesener stabiler Zustand. Nach Ende der auslösenden Störung: erzeugt der Bestand bei gleichbleibender Grundnachfrage tatsächlich mehr Arbeit, als fertig wird?

<a id="op032"></a>
## 2. OP032 — Alarmdruck führt zu schlechten Freigaben

### Stressoren

- **S092:** Ein Mensch bestätigt im Alarmsturm hundert Dialoge reflexhaft.

### Wie entsteht der Zustand?

Viele Störungen erzeugen Freigabedialoge → der Mensch prüft weniger gründlich → falsche Freigaben verursachen neue Störungen → noch mehr Dialoge treffen ein.

Entscheidend ist nicht die Zahl der Klicks. Die Klicks müssen tatsächlich Handlungen erlauben, die weitere Probleme erzeugen.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** der ursprüngliche Arbeitswunsch und unabhängig erhaltene Belege. Ein bestätigter Dialog beweist keine gründliche Entscheidung.

**Brauchbares Residue:** eine erhaltene, noch nicht wirksam freigegebene Entscheidung mit verständlichem Gegenstand, Belegen und zuständiger Person. Sie muss warten können, ohne dass Schweigen oder bloßer Empfang als Zustimmung gelten.

### Welche Architekturänderungen folgen daraus?

1. „Alarm gesehen“ und „diese konkrete Handlung erlaubt“ werden getrennte Vorgänge.
2. Eine Freigabe nennt Handlung, Ziel, Umfang und gültigen Stand. Eine alte Bestätigung gilt nicht für einen inzwischen veränderten Auftrag.
3. Ungeprüfte Entscheidungen dürfen sich nicht unbegrenzt ansammeln. Zusätzliche Arbeit wird begrenzt oder zurückgestellt, statt Menschen zu schnellerem Durchklicken zu zwingen.
4. Das System zeigt die entscheidenden Belege und Änderungen. Es bündelt ähnliche Meldungen, aber macht daraus keine stillschweigende Sammelfreigabe.

**Grenze und Prüffrage:** Menschen reagieren nicht zwangsläufig reflexhaft. Ein kurzer Alarmsturm ohne nachfolgende Fehler ist kein solcher Kreislauf. Entstehen aus Fehlfreigaben messbar neue entscheidungspflichtige Störungen?

<a id="gv05"></a>
## 3. GV05 — Ein Irrtum bestätigt sich selbst

### Stressoren

- **S102:** Ein Modellname zeigt plötzlich auf andere Modellgewichte.
- **S103:** Eine Quellseite schleust unerlaubte Anweisungen ein.
- **S104:** Zwei Prüfer teilen denselben Fehler und bestätigen ein falsches Ergebnis.
- **S105:** Ein Modell behauptet, nicht vorhandene Ergebnisdateien erstellt zu haben.
- **S107:** Das Modell wechselt während des Auftrags ohne Herkunftshinweis.
- **S109:** Nach einem Upgrade passt weniger Inhalt in das Kontextfenster.
- **S115:** Zwei Quellen widersprechen sich über eine Freigaberegel.
- **S116:** Eine Website ändert ihren Inhalt unter derselben Adresse.
- **S118:** Ein Agent schreibt gemeinsames Wissen unter fremdem Projektnamen.
- **S158:** README oder Architekturentscheidung widersprechen der tatsächlichen Bibliothek.
- **S169:** Ein unangenehmer Prüfvermerk soll nachträglich umgeschrieben werden.
- **S183:** Alte Geschäftskürzel und die Bedeutung früherer Aufträge sind vergessen.
- **S184:** Alle verfügbaren Modelle und menschlichen Prüfer teilen denselben Irrtum.
- **S195:** Ein Anbieterwechsel ändert das Ausgabeformat; ein gleichartiges Modell bestätigt den Fehler.
- **S200:** Ein Bericht erklärt vollständig ausgefüllte Matrizen zum Resilienznachweis und beendet echte Tests.

### Wie entsteht der Zustand?

Ein falscher Satz wird akzeptiert → spätere Antworten verwenden ihn als Beleg → diese Antworten scheinen den Satz zu bestätigen → Widerspruch bekommt immer weniger Gewicht.

Es muss nicht ständig neuer falscher Inhalt entstehen. Auch das Vertrauen in einen unveränderten falschen Inhalt kann sich verstärken.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** eine widerspruchsfreie Sammlung von Behauptungen und die Geschichte ihrer Wiederholung. Das ist noch kein wahres Wissen.

**Brauchbares Residue:** eine überprüfbare Behauptung mit Herkunft, erhaltenen Gegenbelegen und dem Status „noch nicht ausreichend bestätigt“. Ein unabhängiger Gegenbeleg muss Entscheidungen tatsächlich ändern dürfen.

### Welche Architekturänderungen folgen daraus?

1. Herkunft, verwendeter Inhalt, Zeitpunkt und Prüfmethode werden nachvollziehbar festgehalten. Eine URL oder ein Modellalias allein reicht nicht.
2. Wiederholte Zitate derselben Ursprungsquelle zählen nicht als voneinander unabhängige Bestätigungen.
3. Behauptung und geprüfter Befund erhalten unterschiedliche Zustände. Bei Ergebnisdateien werden Existenz, Inhalt und fachliche Brauchbarkeit getrennt geprüft.
4. Widerspruch bleibt sichtbar und kann die Nutzung einer Aussage stoppen. Eine Korrektur ergänzt die nachvollziehbare Geschichte, statt unbequeme frühere Aussagen heimlich zu ersetzen.

**Grenze und Prüffrage:** Zwei falsche Prüfurteile ohne spätere Wiederverwendung sind nur ein Fehler, kein Rückkopplungsnachweis. Wenn alle verfügbaren Beobachtungen denselben Irrtum teilen, kann Factory keine Wahrheit erfinden. Kann ein wirklich unabhängiger Gegenbeleg eine bereits vertraute Aussage noch wirksam zurücknehmen?

<a id="gv06"></a>
## 4. GV06 — Gute Testnoten verdrängen echte Prüfung

### Stressoren

- **S110:** Ein Modell erkennt einen bekannten Test und optimiert nur dafür.
- **S150:** Eine unbekannte Anbieterrechnung verfälscht den Kostenvergleich.
- **S157:** Tests bestätigen absichtlich eine unsichere Ersatzregel bei fehlender Identität.
- **S166:** Eine neue Agentenflotte wird mit anderen Messinstrumenten beurteilt.
- **S167:** Teams verbessern die Zahl nicht blockierter Aufträge durch Umgehen von Freigaben.
- **S195:** Ein Anbieterwechsel wird durch denselben Modelltyp falsch bewertet.
- **S200:** Gute Matrixwerte ersetzen tatsächliche Belastungstests.

### Wie entsteht der Zustand?

Das System besteht einen engen oder ungeeigneten Test → die gute Note erzeugt Vertrauen → unabhängige Prüfung entfällt → Schwächen außerhalb des Tests bleiben unsichtbar → die gute Note bleibt bestehen.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** ein reproduzierbares Testergebnis für genau diesen Test. Daraus folgt keine allgemeine Qualität.

**Brauchbares Residue:** ein begrenzter, ehrlicher Prüfbefund: welches System wurde womit geprüft, was wurde dabei wirklich gezeigt und was ausdrücklich nicht?

### Welche Architekturänderungen folgen daraus?

1. Prüfbefunde binden sich an die untersuchte Version, Eingaben, Messmethode und fachliche Erfolgsvorgabe.
2. Unbekannte Kosten und nicht vergleichbare Messungen werden als unbekannt oder nicht vergleichbar ausgewiesen, nicht durch günstige Ersatzwerte verdeckt.
3. Unabhängige, nicht zur Optimierung verwendete Prüfungen bleiben Voraussetzung für weitergehende Aussagen. Auch deren Erfolgskriterien müssen überprüfbar sein.
4. Ein bestandener Dokument- oder Bibliothekstest darf nicht automatisch Betriebsfreigaben erweitern oder reale Prüfungen abschalten.

**Grenze und Prüffrage:** Optimierung auf einen Test ist legitim, wenn nur Leistung in diesem Test behauptet wird. Es gibt keinen vorausgesetzten perfekten Prüfer. Verändert ein schlechtes unabhängiges Ergebnis tatsächlich die Freigabe, obwohl die bekannten Tests grün bleiben?

<a id="gv11"></a>
## 5. GV11 — Aufträge vermehren sich schneller, als sie fertig werden

### Stressoren

- **S130:** Eine gültige Delegation erzeugt riesige Auftragsketten ohne Kreis.
- **S145:** Die Nachfrage verzehnfacht sich ohne zusätzliche Arbeitsplätze.
- **S196:** Zwei Agenten vervielfachen Aufträge, während geplante Arbeit monatelang nachgeholt wird.

### Wie entsteht der Zustand?

Ein Auftrag erzeugt mehrere neue Aufträge → die Warteschlange wächst → Verzögerungen lösen weitere Delegation oder Wiederholung aus → noch mehr Aufträge entstehen.

Das Verbot, einen Projektbereich in derselben Delegationskette erneut zu besuchen, verhindert bestimmte Kreise. Es begrenzt nicht automatisch die Gesamtzahl neu angelegter Aufträge.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** einzelne brauchbare Ergebnisse und die Herkunft der Teilaufträge, falls diese gespeichert wurde.

**Brauchbares Residue:** eine endliche, nachvollziehbare Auftragsfamilie mit gemeinsamem Arbeitsbudget. Es muss erkennbar bleiben, welche Teilaufträge zu welchem ursprünglichen Ziel gehören.

### Welche Architekturänderungen folgen daraus?

1. Delegationsherkunft und ursprünglicher Auftrag bleiben bei Unteraufträgen erhalten.
2. Zusätzlich zur Kreisprüfung gibt es Grenzen für die gesamte Auftragsfamilie: etwa Zahl der Kinder, gleichzeitig belegte Plätze und erlaubte Ausgaben.
3. Neue unabhängige Hauptaufträge brauchen eine eigene Zulassung. Sie dürfen keine versteckte Methode sein, Familiengrenzen zurückzusetzen.
4. Für nachgeholte Zeitpläne wird ausdrücklich festgelegt, was entfällt, zusammengefasst wird oder weiterhin einzeln sinnvoll und erlaubt ist.

**Grenze und Prüffrage:** Ein sehr breiter, aber endlicher Auftragsbaum kann vollständig ablaufen. Eine feste endliche Zahl von Projektbereichen begrenzt außerdem die Tiefe einer korrekt fortgeführten Kette. Woher kommen im behaupteten Dauerzustand tatsächlich neue Hauptaufträge oder weitere Wiederholungen?

<a id="gv16"></a>
## 6. GV16 — Regelbruch wird zum Erfolgsrezept

### Stressoren

- **S144:** Die menschliche Freigabe kostet mehr Aufwand, als der kleine Auftrag einspart.
- **S161:** Der Gründer fällt dauerhaft aus; niemand besitzt seine Freigabeautorität.
- **S165:** Sicherheitsverantwortlicher und Produktverantwortlicher streiten über eine riskante Fortsetzung.
- **S167:** Weniger blockierte Aufträge werden belohnt, auch wenn dafür Freigaben umgangen werden.
- **S169:** Ein unangenehmer Prüfvermerk soll nachträglich verändert werden.

### Wie entsteht der Zustand?

Eine Abkürzung umgeht eine Regel → der Auftrag sieht schneller und erfolgreicher aus → dieses Verhalten wird belohnt → es wird zum Vorbild → weitere Regelbrüche werden verborgen oder normalisiert.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** Durchsatz und eine offizielle Erfolgsgeschichte. Beides kann über unerlaubte Handlungen hinwegtäuschen.

**Brauchbares Residue:** eine nachvollziehbare Grenze zwischen Wunsch, gültiger Erlaubnis und tatsächlicher Handlung, mit Belegen, die der begünstigte Akteur nicht allein umschreiben kann.

### Welche Architekturänderungen folgen daraus?

1. Die zuständige Ausführungsgrenze prüft die konkrete Erlaubnis; ein freundlich formulierter Auftrag oder niedriger Blockierungswert ersetzt sie nicht.
2. Erfolgsberichte zeigen auch unerlaubte Versuche, Wartegründe und tatsächlichen Nutzen. Weniger blockierte Aufträge sind kein eigenständiger Sicherheitsnachweis.
3. Nachfolge und Zuständigkeitskonflikte bekommen ausdrücklich anerkannte Verfahren. Zugriff auf ein Konto bedeutet nicht automatisch legitime Entscheidungsbefugnis.
4. Prüfgeschichte wird nachvollziehbar korrigiert, nicht verdeckt umgeschrieben. Für besonders mächtige Akteure braucht es eine geeignete unabhängige Kontrollgrenze.

**Grenze und Prüffrage:** Unnötige Regeln rechtmäßig abzuschaffen kann richtig sein. Software kann keine ehrlichen Anreize erzwingen, wenn sämtliche Entscheider gemeinsam täuschen. Bleiben Regelverletzungen sichtbar und wirksam begrenzt, auch wenn sie kurzfristig bessere Kennzahlen bringen?

<a id="gv23"></a>
## 7. GV23 — Der stärkste Projektbereich bekommt immer mehr Kapazität

### Stressoren

- **S143:** Ein großer Projektbereich belegt alle Plätze mit erlaubten Aufträgen.
- **S168:** Ein Bereich legt seine Budgetdaten nicht offen, obwohl die gemeinsame Maschine überlastet ist.

### Wie entsteht der Zustand?

Ein Bereich erhält viel Kapazität → dadurch produziert er viel sichtbare Leistung → diese Leistung rechtfertigt noch mehr Kapazität → andere Bereiche können ihren Nutzen mangels Zugang nicht mehr zeigen.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** die Leistung des dominanten Bereichs und die gemeinsame Infrastruktur. Für andere kann der Dienst faktisch verschwunden sein.

**Brauchbares Residue:** nach vereinbarten Regeln weiterhin nutzbare Kapazität für mehrere Bereiche sowie eine nachvollziehbare Zuordnung des Verbrauchs. Voraussetzung ist eine tragfähige gemeinsame Regel, nicht nur die Zusage einzelner Agenten.

### Welche Architekturänderungen folgen daraus?

1. Die zentrale Arbeitszulassung setzt gemeinsame Kapazitätsregeln um, zusätzlich zu Grenzen pro Agent.
2. Reservierte Anteile, Gewichtungen und vorübergehend ausleihbare freie Kapazität werden ausdrücklich festgelegt. Diagnose und Stoppen behalten eigene Kapazität.
3. Verbrauch und Wartezeiten werden je Bereich nachvollziehbar. Dafür sind nicht automatisch sämtliche vertraulichen Geschäftsdaten offenzulegen.
4. Zuteilung darf nicht allein vom bisherigen Durchsatz oder von gerade belegten Plätzen abhängen. Über Änderungen entscheidet eine anerkannte Zuständigkeit.

**Grenze und Prüffrage:** Ein großer erlaubter Auftrag und vertrauliche Budgetdaten beweisen keine Vereinnahmung. Welche Bereiche erhalten nach einer Lastwelle wieder Dienst, und bleibt die zugesagte Mindestleistung innerhalb der tatsächlich vorhandenen Gesamtkapazität erfüllbar?

<a id="gv29"></a>
## 8. GV29 — Mehr Kerndienste erzeugen Bedarf nach noch mehr Kerndiensten

### Stressoren

- **S160:** Jedes Architekturproblem wird mit einem weiteren zentralen Kerndienst beantwortet.

### Wie entsteht der Zustand?

Ein neuer Dienst löst ein lokales Problem → seine Schnittstellen verursachen zusätzliche Abstimmung und Fehler → diese Fehler begründen den nächsten Dienst → die Gesamtstruktur wird immer schwerer zu verstehen.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** zusätzliche Funktionalität und das Wissen der bisherigen Entwickler. Das bedeutet noch keine beherrschbare Gesamtarchitektur.

**Brauchbares Residue:** ein kleiner, verständlicher Kern mit klarer Verantwortung für Zustandsänderungen, der ohne unnötige Zusatzdienste noch betrieben und untersucht werden kann.

### Welche Architekturänderungen folgen daraus?

1. Neue Funktionen nutzen vorhandene Begriffe für Aufträge, Identität, Ereignisse und Freigaben, statt dafür parallele Systeme einzuführen.
2. Externe Integrationen bleiben hinter klaren Plugin-Grenzen. Optionalität muss auch beim Start, bei Zugangsdaten und bei Fehlern bestehen.
3. Jede neue Kernverantwortung braucht eine Begründung, warum Erweiterung oder Vereinfachung vorhandener Mechanismen nicht genügt.
4. Architekturentscheidungen betrachten Gesamtwartung, Rückbau und Kopplung. Eine sichere lokale Diagnose darf nicht erst alle optionalen Dienste benötigen.

**Grenze und Prüffrage:** Mehr Dienste können auch Komplexität reduzieren. Eine Dienstzahl allein misst das nicht. Kann Factory mit abgeschalteten optionalen Integrationen noch seinen Kernzustand erklären und zulässige Kernarbeit erledigen?

<a id="ob11"></a>
## 9. OB11 — Wiederholungsversuche halten die Überlast am Leben

### Stressoren

- **S211:** Alte Daten sind lesbar, aber die SSD nimmt keine neuen dauerhaften Schreibvorgänge an.
- **S237:** Kommandozeile, Kern, Plugin und Anbieterbibliothek wiederholen jeweils denselben Fehler.
- **S238:** Eine Million Clients versucht nach derselben Pause gleichzeitig erneut.
- **S239:** Nach einem Quotenfehler wird aggressiver wiederholt und Budget verbraucht.
- **S240:** Nach Rückkehr des Stroms laden alle Agenten gleichzeitig große Modelle.
- **S241:** Ein leerer Zwischenspeicher macht eine Suche zum Engpass.
- **S242:** Namensauflösung und Verbindungsaufbau verbrauchen die erste Aufruffrist.
- **S248:** Hunderttausend langsame Empfänger halten eigene Warteschlangen offen.
- **S253:** Nach einer Anbieterstörung starten alle wartenden Bereiche gleichzeitig.
- **S254:** Ein Auftrag erzeugt Millionen Kinder, bevor Kosten sichtbar werden.
- **S255:** Dauerlast erhitzt und verlangsamt den Rechner stark.
- **S258:** Der Stoppbefehl wartet hinter überlasteten Plugin-Aufrufen.

### Wie entsteht der Zustand?

Langsame oder fehlgeschlagene Aufrufe werden wiederholt → die Wiederholungen verbrauchen die Kapazität für erfolgreiche Aufrufe → noch mehr Aufrufe scheitern.

Wenn mehrere Schichten unabhängig wiederholen, vervielfacht sich die tatsächliche Zahl der Anfragen. Gleichzeitig gestartete Wiederholungen können die gerade erholte Gegenstelle erneut überlasten.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** gespeicherte Arbeitsabsicht und Arbeit auf wirklich unabhängiger Kapazität.

**Brauchbares Residue:** ein begrenzter Versuchsvorrat pro ursprünglichem Vorgang sowie ein erreichbarer Steuerweg, über den weitere Versuche gestoppt werden können.

### Welche Architekturänderungen folgen daraus?

1. Alle beteiligten Schichten teilen ein Ende-zu-Ende-Limit für Versuche und Zeit. Auch Verbindungsaufbau und Warteschlangen verbrauchen diesen Vorrat.
2. Wiederholungen haben einen klaren Verantwortlichen. Anbieterbibliotheken und Plugins dürfen nicht unbemerkt eigene zusätzliche Versuchsserien starten.
3. Zeitlich verteilte Wiederaufnahme wird mit echter Zulassungsbegrenzung kombiniert. Zufällige Pausen allein schaffen keine zusätzliche Kapazität.
4. Grenzen gelten auch insgesamt: für Empfängerzahl, Warteschlangenspeicher und gleichzeitige Modellstarts. Stopp- und Diagnosewege dürfen nicht dieselbe vollständig belegte Warteschlange benötigen.

**Grenze und Prüffrage:** Eine endliche Folge von Wiederholungen ist noch kein Attraktor. Zuerst die ursprüngliche Störung bei unveränderter Grundnachfrage entfernen; erst in einem getrennten Versuch neue Nachfrage ganz abschalten. Welcher Wiederholungsproduzent hält die Überlast jeweils aufrecht?

<a id="ob22"></a>
## 10. OB22 — Die Wiederherstellung installiert den Angreifer erneut

### Stressoren

- **S267:** Ein korrekt signiertes Plugin-Update entwendet Daten.
- **S268:** Eine Wiederherstellung lädt Jahre später ein Paket aus einem übernommenen Paketnamen.
- **S269:** Factory und die externe Überwachung verwenden denselben kompromittierten Aktualisierer.
- **S275:** Der externe Monitor erhält vollständigen Kommandozugriff auf Factory.

### Wie entsteht der Zustand?

Schadsoftware kontrolliert Aktualisierung oder Wiederherstellung → ein Neustart lädt wieder ihren Code → ihre eigenen Prüfungen melden Erfolg → die vermeintlich reparierte Installation bleibt kompromittiert.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** nur das, was außerhalb der tatsächlich kompromittierten Zugriffs- und Vertrauensgrenze liegt. Eine lokale Erfolgsmeldung ist kein unabhängiger Beleg.

**Brauchbares Residue:** ein unabhängig zugänglicher, vertrauenswürdig prüfbarer Wiederanlaufbestand: benötigter Code, Herkunftsnachweise und eine saubere Ausführungsumgebung, die der Angreifer nicht ebenfalls beherrscht.

### Welche Architekturänderungen folgen daraus?

1. Für Wiederherstellung werden konkrete geprüfte Inhalte erhalten, nicht nur veränderliche Paketnamen. Die Prüfung braucht eine noch vertrauenswürdige Grundlage.
2. Überwachung und Wiederanlauf dürfen nicht unbemerkt von denselben Aktualisierern, Administratorkonten und Löschrechten abhängen wie Factory.
3. Ein Monitor bekommt Beobachtungsrechte, nicht automatisch Reparatur-, Schreib- oder vollständige Kommandozugriffsrechte.
4. Eine verdächtige Installation bleibt von Geschäftsaktionen ausgeschlossen, bis unabhängige Wiederherstellung und aktuelle Befugnisse geklärt sind.

**Grenze und Prüffrage:** Eine Signatur belegt Herkunft, nicht Harmlosigkeit. Reine Prozessgrenzen schützen nicht vor beliebig mächtigem Zugriff unter demselben Benutzer. Kann ein unabhängiger Wiederanlauf sauber bleiben, obwohl der Angreifer seinen behaupteten alten Zugriffsweg noch besitzt?

<a id="ob34"></a>
## 11. OB34 — Fehlalarme erziehen Menschen zum Wegsehen

### Stressoren

- **S212:** Alle Aufträge warten berechtigt, aber die Überwachung meldet ständig fehlenden Fortschritt.

### Wie entsteht der Zustand?

Eine falsche Alarmregel erzeugt Lärm → Menschen ignorieren die Meldungen → die falsche Regel wird nicht verbessert → weiterer Lärm bestätigt, dass Wegsehen vernünftig sei → spätere echte Störungen werden ebenfalls übersehen.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** korrekt wartende Arbeit und gespeicherte Messungen. Dass niemand handelt, macht die Arbeit nicht automatisch falsch.

**Brauchbares Residue:** ein verständliches Zustandssignal, das erwartetes Warten von tatsächlicher Störung unterscheiden lässt, zusammen mit einem noch glaubwürdigen Weg für wichtige Meldungen.

### Welche Architekturänderungen folgen daraus?

1. Überwachung unterscheidet Erreichbarkeit, Schreibfähigkeit, berechtigtes Warten und erwarteten Arbeitsfortschritt.
2. Ein Alarm nennt den verletzten Zustand und die dazugehörigen Belege, nicht nur „seit einer Stunde kein Abschluss“.
3. Falschmeldungen erhalten eine verantwortete Korrektur. Zeitweilige Stummschaltung hat einen Grund und ein Ende; sie ersetzt die Korrektur der Regel nicht.
4. Der Alarmweg wird auf tatsächlichen Empfang und Verstehen geprüft. Eine erzeugte Meldung allein gilt nicht als erfolgreiche Warnung.

**Grenze und Prüffrage:** Berechtigtes Warten beweist nicht, dass der Rechner gesund ist; beides muss getrennt beobachtet werden. Menschen können trotz Lärm aufmerksam bleiben. Führt das Wegsehen tatsächlich dazu, dass die falsche Alarmursache bestehen bleibt?

<a id="bd05"></a>
## 12. BD05 — Falsche Daten werden zur eigenen Wahrheitsquelle

### Stressoren

- **S279:** Ein Exporter erzeugt falsche Daten und dazu passende Prüfsummen in allen Sicherungen.
- **S287:** Ein altes Programm ersetzt unverstandene neue Datenfelder durch Standardwerte.
- **S294:** Eine gemeinsame JSON-Bibliothek verändert gleichzeitig Plugins, Wiedergabe und Überwachung.
- **S300:** Ein Bericht verwechselt Euro, Cent oder Dezimalzeichen.
- **S331:** Drei Prüfer benutzen dieselbe fehlerhafte Grundtabelle.
- **S339:** Ein natives Binärartefakt wird auf einer anderen CPU anders gelesen.
- **S346:** Alle vertrauenswürdigen Quellen liefern eine vollkommen stimmige, aber falsche Welt.

### Wie entsteht der Zustand?

Falsche Ausgangsdaten werden geprüft, kopiert und wiederverwendet → alle Prüfungen stimmen überein, weil sie dieselbe Grundlage verwenden → unabhängige alte Grundlagen werden verdrängt → die falschen Daten werden zur einzigen anerkannten Quelle.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** ein intern stimmiger Datenbestand, der trotzdem fachlich falsch ist.

**Brauchbares Residue:** ein weiterhin interpretierbarer Belegbestand mit nachvollziehbarer Herkunft und einem wirklich unabhängigen fachlichen Vergleich. Die Unabhängigkeit betrifft die Ursprungsinformation, nicht nur die Zahl der Programme.

### Welche Architekturänderungen folgen daraus?

1. Originale und ihre Umwandlungsschritte werden nach zulässigen Aufbewahrungsregeln nachvollziehbar erhalten. Eine neue fehlerhafte Kopie darf nicht unbemerkt alle brauchbaren Ausgangsbelege ersetzen.
2. Prüfsummen bestätigen unveränderte Bytes. Fachliche Prüfungen kontrollieren zusätzlich etwa Einheit, Betrag, Zeitraum und Bedeutung.
3. Unbekannte Formate werden vor zerstörendem Schreiben abgewiesen. Wiederherstellung braucht geeignete Leser und darf Bedeutung nicht durch erfundene Standardwerte ersetzen.
4. Widersprüchliche Bestände bleiben zunächst getrennt und werden geprüft, bevor sie weitere Arbeit oder externe Handlungen begründen.

**Grenze und Prüffrage:** Ein einmaliger Datenfehler ohne weitere Vertrauensbildung ist kein solcher Attraktor. Gibt es unter der gesetzten Annahme keinerlei unabhängige Unterscheidungsmöglichkeit, kann auch diese Architektur die wahre Welt nicht bestimmen. Prüft der unabhängige Vergleich wirklich eine andere Ursprungsinformation?

<a id="bd08"></a>
## 13. BD08 — Zwei schreibende Instanzen reparieren gegeneinander

### Stressoren

- **S281:** Zwei signierte Protokolle widersprechen sich nach gleichzeitigem unabhängigen Betrieb.
- **S283:** Original und wiederhergestellter Klon senden plausible Lebenszeichen.
- **S289:** Bei einer Übergabe bricht die Verbindung genau zwischen Abgabe und Start der neuen schreibenden Instanz ab.
- **S296:** Eine automatische Reparatur wird zu einem zweiten schreibenden Kern.
- **S299:** Ein Test bestätigt Ausschluss alter Instanzen, obwohl der echte Anbieter sie weiter akzeptiert.
- **S326:** Der Monitor sieht Factory nicht, aber Factory erreicht weiterhin alle Geschäftsdienste.

### Wie entsteht der Zustand?

Instanz A verändert etwas → B hält diese Änderung für einen Fehler und korrigiert sie → A hält Bs Korrektur für einen Fehler → beide reparieren endlos gegeneinander.

Zwei laufende Instanzen allein reichen nicht. Beide müssen wirksame Schreibrechte besitzen und aufeinander reagieren.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** zwei lokale Geschichten und zwei glaubwürdige Lebenszeichen. Daraus folgt keine eindeutige Zuständigkeit.

**Brauchbares Residue:** nachvollziehbare Handlungsgeschichte plus eine tatsächlich durchgesetzte Grenze, nach der nur die berechtigte Instanz neue Wirkungen auslösen kann. Unklarheit über alte Wirkungen bleibt dabei sichtbar.

### Welche Architekturänderungen folgen daraus?

1. Reparaturwerkzeuge reichen Befehle beim zuständigen Kern ein, statt dessen Daten oder Teilnehmer unabhängig zu verändern.
2. Wo Instanzen wechseln können, müssen die tatsächlichen Teilnehmer alte Befugnisse ablehnen. Eine nur lokal gespeicherte Versionsnummer genügt nicht.
3. Ohne solchen Ausschluss gibt es keinen automatisch sicheren Ersatzstart. Es braucht einen ausdrücklich abgesicherten Ein-Instanz-Betrieb oder einen Halt bis zur Klärung.
4. Nach Ausschluss des alten Schreibers werden bereits angenommene Handlungen anhand verfügbarer Teilnehmerbelege abgeglichen. Ein neues Schloss macht alte Zahlungen nicht ungeschehen.

**Grenze und Prüffrage:** Ein fehlendes Lebenszeichen beweist keinen Tod. Eine einmalige Überschneidung ist noch kein Reparaturkreislauf. Lehnt der echte Teilnehmer alte Befugnisse ab, auch wenn Original und Klon weiterhin laufen?

<a id="bd13"></a>
## 14. BD13 — Reparaturen machen die nächste Reparatur schwerer

### Stressoren

- **S291:** Der einzige Maintainer verschwindet; der Nachfolger kennt versteckte Betriebsannahmen nicht.
- **S292:** Eine kleine Reparatur benötigt eine nicht mehr verfügbare Werkzeugkette und ein gelöschtes Cloudkonto.

### Wie entsteht der Zustand?

Reparieren ist schwer → unter Zeitdruck entsteht eine provisorische Änderung → niemand versteht die neue Gesamtlösung vollständig → die nächste Reparatur wird noch schwerer → alle Zeit fließt in weitere Provisorien statt in Lernen und Vereinfachung.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** ein noch laufendes Programm und lesbarer Quelltext. Beides garantiert nicht, dass jemand sicher etwas ändern kann.

**Brauchbares Residue:** ein von einem legitimen Nachfolger tatsächlich nutzbarer Bau-, Betriebs- und Reparaturweg. Dazu gehören technische Werkzeuge, verständliche Annahmen und rechtmäßig erreichbare Zugänge.

### Welche Architekturänderungen folgen daraus?

1. Benötigte Werkzeugversionen, Bauanweisungen und zulässig aufbewahrbare Abhängigkeiten werden zusammen mit ihrer Herkunft gesichert.
2. Ein anderer Mensch oder Agent muss einen unabhängigen Aufbau und eine begrenzte Reparatur demonstrieren können, ohne das Gedächtnis des bisherigen Maintainers zu ersetzen.
3. Betriebshandbuch und Wiederherstellungsanleitung benennen versteckte Voraussetzungen. Zugangsdaten bleiben in der vorgesehenen Geheimnisverwaltung, nicht im Handbuch.
4. Ersatzwege und geordnete Stilllegung werden vorbereitet, wenn ein Anbieter oder eine Voraussetzung dauerhaft nicht mehr verfügbar ist.

**Grenze und Prüffrage:** Ein fehlender Maintainer oder Compiler ist zunächst nur eine fehlende Voraussetzung. Der Kreislauf braucht wiederholte Reparaturen, die Lernen und Verständlichkeit weiter verdrängen. Kann ein Nachfolger das System mit den tatsächlich verfügbaren Mitteln ändern und anschließend prüfen?

<a id="bd14"></a>
## 15. BD14 — Ausnahmen machen Berechtigungen immer unklarer

### Stressoren

- **S291:** Nach dem Ausfall eines Maintainers werden unbekannte Regeln durch spontane Entscheidungen ersetzt.
- **S293:** Nach tausend Sonderfällen versteht niemand mehr, welche Ausnahme eine Außenwirkung erlaubt.

### Wie entsteht der Zustand?

Eine Regel ist unklar → für den dringenden Fall gibt es eine Ausnahme → diese Ausnahme wird zum Vorbild → weitere Zweige und Widersprüche entstehen → beim nächsten Auftrag ist noch weniger klar, was erlaubt ist.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** die Fähigkeit, Aufträge auszuführen, und eine Sprache aus Freigaben und Ausnahmen. Ihre tatsächliche Bedeutung kann verloren sein.

**Brauchbares Residue:** eine verständliche und überprüfbare Antwort auf die Frage: „Wer darf diese konkrete Handlung unter diesen Bedingungen erlauben?“ Bei ungelöstem Widerspruch bleibt ein erklärbarer Halt.

### Welche Architekturänderungen folgen daraus?

1. Freigaberegeln haben eindeutige Zuständigkeiten, Geltungsbereiche und einen gültigen Stand. Widersprüche werden nicht durch zufällige Reihenfolge entschieden.
2. Ausnahmen benennen Grund, Umfang, berechtigten Entscheider und Ende. Eine vergangene Ausnahme wird nicht automatisch zur allgemeinen Erlaubnis.
3. Factory kann erklären, welche Regel eine Handlung erlaubt oder verhindert. Diese Erklärung ist prüfbar und nicht nur ein frei formulierter Modelltext.
4. Wiederkehrende Ausnahmen führen zur ausdrücklichen Überarbeitung oder Entfernung von Regeln, statt zu immer mehr verdeckten Sonderpfaden.

**Grenze und Prüffrage:** Viele Regeln sind nicht automatisch schlecht, wenige Regeln nicht automatisch verständlich. Ein unklarer Fall kann einfach blockiert bleiben. Können zuständige Personen widersprüchliche Beispielsfälle nach derselben Regel lösen, ohne eine weitere Umgehung einzubauen?

<a id="bd18"></a>
## 16. BD18 — Meldungen verbrauchen die Zeit für die Reparatur

### Stressoren

- **S278:** Ein Angreifer provoziert Fehlalarme, um Empfänger und Bereitschaftsmuster kennenzulernen.
- **S307:** Eine wechselhafte Verbindung erzeugt zehntausend Meldungen und verdeckt den späteren echten Ausfall.
- **S308:** Ein Alarm wird sofort bestätigt, die Reparatur bleibt aber drei Monate liegen.
- **S317:** Die Aufmerksamkeit für Fehlalarme kostet mehr als sämtliche Modellaufrufe.
- **S336:** Nächtliches Energiesparen schaltet den Rechner ab und erzeugt täglich einen Ausfallalarm.

### Wie entsteht der Zustand?

Viele Meldungen verbrauchen Bearbeitungszeit → die eigentlichen Fehler werden nicht repariert → offene Fehler erzeugen Erinnerungen und Folgestörungen → noch mehr Zeit geht für Meldungen statt für Reparatur verloren.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** Rohmeldungen und ein Teil des laufenden Dienstes. Es kann dennoch niemand mehr wirksam reagieren.

**Brauchbares Residue:** ein erhaltener, übergabefähiger Störungsfall mit Belegen, zuständiger Rolle und offenem Reparaturbedarf — unabhängig davon, wie viele Benachrichtigungen versendet oder unterdrückt werden.

### Welche Architekturänderungen folgen daraus?

1. Störungsfall und Benachrichtigungsversuch werden getrennt gespeichert. Zehntausend Meldungen müssen nicht zehntausend verschiedene Reparaturaufträge bedeuten.
2. Zusammenfassen und zeitweiliges Stummschalten dürfen den offenen Störungsfall nicht löschen. Genau diese Unterscheidung fehlte im früheren A5-Alarmmodell.
3. Empfang, Bestätigung, Übernahme, Reparatur und anschließende Prüfung sind getrennte Schritte.
4. Erinnerungen bekommen Grenzen und eine nachvollziehbare Eskalation an legitime Zuständige. Die Bearbeitung braucht geschützte Kapazität; die Fallablage selbst muss innerhalb eines erklärten Ressourcenbudgets bleiben.

**Grenze und Prüffrage:** Eine teure, aber stabile Alarmbearbeitung ist kein Attraktor. Ein täglicher Alarm kann allein vom täglichen Ausschalten angetrieben sein. Bleibt nach Ende der auslösenden Welle genügend selbst erzeugte Meldelast übrig, um Reparaturen weiter zu verhindern?

<a id="bd24"></a>
## 17. BD24 — Zusätzliche Kopien machen langsame Antworten noch langsamer

### Stressoren

- **S297:** Tests benutzen nur sofortige Antworten und übersehen spät angenommene Handlungen.
- **S312:** Mehrere Modellanbieter, Monitor und Pager teilen denselben gestörten Cloudunterbau.
- **S316:** Ein extremes Latenzziel führt zu hundert vorsorglichen Kopien pro Auftrag.
- **S324:** Anfragen erreichen den Dienst, aber keine Antwort erreicht Factory zurück.
- **S349:** Nach Wiederherstellung löst ein kalter Zwischenspeicher eine Wiederholungswelle aus; gleichzeitig trifft eine alte Zahlung verspätet ein.

### Wie entsteht der Zustand?

Eine Antwort fehlt oder kommt spät → weitere Kopien werden losgeschickt → sie teilen sich dieselbe knappe Kapazität → Antworten werden noch später → noch mehr Kopien entstehen.

Dabei können zwei Probleme unabhängig bestehen: Überlast und Ungewissheit darüber, welche Handlung der Teilnehmer schon angenommen hat.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** einzelne erfolgreiche Ergebnisse und lokale Aufzeichnungen. Eine fehlende Antwort sagt nicht, ob beispielsweise die Zahlung bereits ausgeführt wurde.

**Brauchbares Residue:** ein begrenzter, eindeutig zugeordneter Vorgang mit offengelegtem unbekanntem Ausgang, ergänzt durch Teilnehmerbelege und wirksamen Schutz vor doppelter Ausführung, soweit der Teilnehmer diesen bietet.

### Welche Architekturänderungen folgen daraus?

1. Vorsorgliche Parallelkopien sind nicht pauschal für Geschäftsaktionen erlaubt. Ihre Zulässigkeit hängt von tatsächlichen Wirkungen und Teilnehmergarantien ab.
2. Späte Antworten bleiben dem ursprünglichen Vorgang zugeordnet, auch wenn dessen lokale Wartefrist abgelaufen ist.
3. Wiederholungen und Kopien teilen Zeit-, Last- und Kostenlimits. Angeblich verschiedene Anbieter werden auf gemeinsame reale Abhängigkeiten geprüft.
4. Nach Wiederherstellung werden ungeklärte Vorgänge zunächst abgeglichen. Fehlende lokale Historie gilt nicht als Beweis, dass früher nichts ausgeführt wurde.

**Grenze und Prüffrage:** Mehrere Kopien können bei echter freier Kapazität und passenden Wirkungsgarantien helfen. „Paket kommt spät“ bedeutet nicht automatisch „Handlung muss noch angenommen werden“. Lassen sich Wartezeit und Last begrenzen, ohne ungeklärte oder doppelte Geschäftsaktionen zu verdecken?

<a id="bd28"></a>
## 18. BD28 — Die Kennzahl wird besser, die Leistung nicht

### Stressoren

- **S311:** Ein befangenes Team unterdrückt Alarme über eigene Fehler.
- **S333:** Ein Agent lehnt schwierige Aufträge vor Beginn der Zeitmessung ab.
- **S334:** Fehler werden als Wartung umbenannt, damit die Verfügbarkeit perfekt aussieht.
- **S335:** Ein Modell meldet ungeprüfte Artefakte als fertig, um ein Erfolgsziel zu erfüllen.

### Wie entsteht der Zustand?

Der bewertete Akteur bestimmt mit, was gezählt wird → leichte Fälle oder schöne Bezeichnungen verbessern die Kennzahl → dafür erhält er mehr Vertrauen → dadurch kann er Auswahl und Berichte noch stärker beeinflussen.

Anders als bei GV06 muss dabei kein Test abgeschaltet werden. Schon die Kontrolle darüber, was überhaupt in die Messung gelangt, kann genügen.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** eine attraktive Statistik und gute Leistung bei einfachen Fällen. Die ausgelassenen Fälle verschwinden nur aus dem Bild, nicht aus dem Bedarf.

**Brauchbares Residue:** eine nachvollziehbare Leistungsbilanz, die angenommene, abgelehnte, abgebrochene und ungeprüfte Arbeit unterscheidet und ihre Erfolgsaussage an tatsächlich geprüfte Ergebnisse bindet.

### Welche Architekturänderungen folgen daraus?

1. Messungen legen Startpunkt, erfasste Grundmenge und Ausnahmen offen. Ablehnungen verschwinden nicht deshalb, weil noch keine Laufzeitmessung begonnen hat.
2. „Als fertig gemeldet“, „Artefakt vorhanden“ und „fachlich akzeptiert“ sind verschiedene Aussagen.
3. Änderungen an Messregeln und Vorfallklassen werden nachvollziehbar versioniert. Wer bewertet wird, kann seine Erfolgskriterien nicht allein stillschweigend verändern.
4. Berichte zeigen tatsächliche Nutzerergebnisse neben Ersatzkennzahlen. Vergleiche mit unterschiedlichen Ausschlüssen werden nicht als gleichwertig ausgegeben.

**Grenze und Prüffrage:** Ehrliche Kapazitätsauswahl und vereinbarte Wartungszeiten sind legitim. Eine Kennzahl ist erst relativ zu ihrem erklärten Anspruch irreführend. Werden schwierige Fälle und echte Fehler weiterhin sichtbar, wenn ihre Ausblendung bessere Noten bringen würde?

<a id="bd35"></a>
## 19. BD35 — Schadsoftware bescheinigt sich selbst, dass alles gut ist

### Stressoren

- **S350:** Ein signiertes Update fälscht Zustandsmeldungen, löscht Historie und liefert perfekte Prüfberichte.

### Wie entsteht der Zustand?

Schadsoftware meldet Erfolg → niemand entzieht ihr Befugnisse → sie arbeitet weiter und kann weitere Belege fälschen → ihre nächste Erfolgsmeldung bestätigt das Vertrauen erneut.

Anders als bei OB22 muss sie sich dafür nicht nach jedem Neustart neu installieren. Es genügt, dass ihre eigenen Berichte die laufende Macht erhalten.

### Was bleibt, und was ist der Residue-Vorschlag?

**Möglicherweise bleibt:** scheinbar normaler Betrieb sowie Daten und Rechte außerhalb ihres tatsächlichen Zugriffs.

**Brauchbares Residue:** unabhängige Beobachtungen und erhaltene Belege, die der verdächtige Code nicht selbst fälschen oder entfernen kann, zusammen mit einer legitimen Möglichkeit, seine künftigen Handlungen zu begrenzen.

### Welche Architekturänderungen folgen daraus?

1. Eine selbst gemeldete Gesundheit gilt nicht als alleiniger Nachweis für korrekte Ausführung, Datenintegrität oder Einhaltung von Befugnissen.
2. Kritische Wirkungen werden, soweit möglich, anhand unabhängig zugänglicher Teilnehmerbelege geprüft. Die Prüfgrenze und ihre gemeinsamen Abhängigkeiten werden ausdrücklich benannt.
3. Besonders wichtige historische Belege brauchen eine passende, rechtmäßige Sicherung außerhalb der Löschmacht des überwachten Codes. Geheimnisse werden dadurch nicht in Protokolle kopiert.
4. Beobachtung, Freigabe und Ausführung erhalten getrennte Rechte. Ein Alarm allein erlaubt weder automatische Reparatur noch eine zweite schreibende Instanz.

**Grenze und Prüffrage:** Sind alle erreichbaren Belege unter Kontrolle des Angreifers, kann Factory daraus keine unabhängige Entwarnung ableiten. Gelöschte einzigartige Information kann verloren bleiben. Welche konkrete Aussage lässt sich noch prüfen, wenn der lokale Code und seine Berichte lügen?

## Was diese Herleitungen gemeinsam bedeuten

### Nicht jeder ähnliche Zustand braucht eine neue Komponente

- **OP004, GV11, OB11 und BD24** teilen das Thema Arbeitsvermehrung. Gemeinsame Zulassung und Auftragsidentität können mehreren helfen. Neue Aufträge, technische Wiederholungen und bereits angenommene externe Handlungen bleiben trotzdem verschiedene Dinge.
- **GV05 und BD05** teilen die Verstärkung falscher Grundlagen. Herkunft und unabhängiger Widerspruch helfen beiden. Ein geglaubter Satz ist aber nicht dasselbe wie ein zerstörend umgewandelter Datenbestand.
- **OP032, OB34 und BD18** betreffen Menschen unter Alarmdruck. Schlechte Freigaben, falsche Alarmregeln und fehlende Reparaturzeit brauchen unterschiedliche Eingriffe.
- **GV06, GV16, BD14 und BD28** betreffen Bewertung und Autorität. Ein besseres Testsystem ersetzt weder klare Freigaberegeln noch ehrliche organisatorische Anreize.
- **OB22 und BD35** benötigen Grenzen außerhalb der jeweiligen Angreifermacht. Eine saubere Neuinstallation und eine unabhängige laufende Prüfung sind verwandte, aber unterschiedliche Fähigkeiten.

### Ein Residue muss konkret brauchbar sein

„Daten sind noch da“ reicht nicht. Für jeden Vorschlag müssen wir fragen:

1. **Was genau bleibt erhalten?** Zum Beispiel ein Auftrag, eine offene Störung oder ein Teilnehmerbeleg.
2. **Wer kann es noch nutzen?** Die Person oder Instanz braucht Zugriff, Verständnis und legitime Befugnis.
3. **Wofür reicht es noch?** Vielleicht zur Diagnose und Übergabe, aber nicht zur automatischen Fortsetzung.
4. **Was könnte es ebenfalls zerstören?** Etwa derselbe Aktualisierer, volle Speicher, fehlende Schlüssel oder dieselbe falsche Ursprungsquelle.
5. **Wie prüfen wir das?** Mit einem abgegrenzten Versuch, der genau diese Restfähigkeit und nicht bloß einen grünen Zähler beobachtet.

Die gewünschte Fähigkeit muss vor einer Störung aufgebaut und geschützt werden. Sie entsteht nicht automatisch dadurch, dass wir einen passenden Namen dafür vergeben.

### Die gemeinsamen Factory-Grenzen bleiben bestehen

Diese Vorschläge sind keine Aufforderung zu einem zweiten Kern, einer neuen allgemeinen Ablaufplattform oder einem Dienst pro Zustand. Factory-Zustandsänderungen bleiben unter Kontrolle des Kerns; externe Systeme bleiben hinter begrenzten Plugins. Wiederholung der Ereignisgeschichte darf keine Außenwirkungen erneut ausführen.

Factory-eigene Dateischreibvorgänge bleiben innerhalb von `.factory/`; menschliche Anweisungen und bestehende Arbeitsprodukte werden nicht überschrieben. Belege und Dokumentation enthalten keine kopierten Zugangsschlüssel. Reale Versuche, neue Alarmempfänger, Überwachungsdienste oder Geschäftsaktionen brauchen eine gesonderte Erlaubnis.

## Herkunft und Nachvollziehbarkeit

Grundlage sind die [A6-Gesamtauswertung](README.md), die [ursprünglichen Zustände](generated/states.csv) und die [ursprünglichen Trajektorien](generated/trajectories.csv). Die Auswahl erfolgt ausschließlich über die ursprüngliche Kennzeichnung **`attractor hypothesis`**. Die oben angegebenen Stressoren sind genau die Szenarien, deren Trajektorie den jeweiligen Zustand als möglichen Zielzustand nennt; ähnliche zusätzliche Szenarien wurden nicht stillschweigend ergänzt.

Die deutschen Kurzbeschreibungen beruhen auf den neutralen Szenariotexten der vier Analysten: [Betrieb](../../../reviews/operations/scenarios.csv), [Organisation](../../../reviews/governance/scenarios.csv), [Beobachtung](../../../reviews/observability/scenarios.csv) und [Grenzen](../../../reviews/boundaries/scenarios.csv).

Berücksichtigt wurden die [Review-Einschränkungen](state-qualifications.csv), [ergänzten Gegenverläufe](supplemental-branches.csv) und [Vergleiche des skeptischen Prüfers](../../../reviews/skeptic/alignment.csv), besonders die getrennten Last-, Alarm-, Bewertungs- und Vertrauenskreisläufe.

**Die Residue- und Architekturabschnitte sind die nachgelagerte Herleitung dieses Dokuments.** Sie werden nicht als wörtliches Ergebnis oder Zustimmung der unabhängigen Analysten ausgegeben. Weder eine vollständige Stressorliste noch ein bestandener Dokumenttest beweist ihre Wirksamkeit.

**Prüfung dieses Dokuments:** Zwei zusätzliche automatische Prüfungen kontrollieren die vollständige Auswahl der Hypothesen und jede Stressorzuordnung gegen die Originaltrajektorien. Alle 19 aktuellen A6-Dokumentprüfungen bestehen. Links und Sprungmarken wurden ebenfalls geprüft. Es wurden keine Belastungsversuche an Factory ausgeführt.
