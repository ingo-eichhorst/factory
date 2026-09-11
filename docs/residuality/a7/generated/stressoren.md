# Alle 350 Stressoren: Zustände, Residues und Architektur

**Bedingte Entwurfsanalyse, keine beobachteten Attraktoren oder bestandenen Resilienztests.**

Jeder Zweig gilt nur unter seinen Bedingungen. `keines` bezeichnet ausdrücklich keine nutzbare Restfähigkeit für den betrachteten Gegenstand; `ungeklaert` eine nicht begründbare Zuordnung. Ein haltender oder geschädigter Zustand ist nicht automatisch ein Attraktor.

Die Voraussetzungen der referenzierten Residues gelten zusätzlich zu den Zweigbedingungen. Anders vorbereitete Schutzentwürfe sind keine Überlebensbehauptung nach wörtlichem Totalverlust. Hashgebundene Nachträge des Koordinators sind bei den betroffenen Zweigen sichtbar.

Die Zweig-IDs sind A7-Analysekennungen, keine Factory-Laufzeitzustände. Mehrere Fähigkeiten können gleichzeitig nötig sein. Eine vollständige Kartenzuordnung beweist keine vollständige Menge aller möglichen Verläufe.

[Zielarchitektur](../zielarchitektur.md) · [Residue-Katalog](residues.md) · [Reviewauflagen](../review-dispositions.md)

<a id="s001"></a>
## S001 — Netzstrom fällt während Commit und Prompt-Übergabe aus

**Ursprung:** operations; A6-Zustände: OP002, OP001, OP014, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s001.b01"></a>
### S001.B01 — Gerät ohne Strom, gespeicherter Auftrag bleibt lesbar nach Rückkehr

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Ausfall vor erfolgreichem Intent-Commit, vorherige Daten bleiben physisch intakt.

**Warum bleibt oder endet der Zustand?** Stromausfall hält Ausführung an; nach Rückkehr endet die Unterbrechung ohne Wiederholung eines auf diesem Pfad nie gerufenen Writers.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Der vorherige Taskkern ist nach Wiederzugang nutzbar, nicht die uncommittete Berechnung. C1 stützt nur die Commit-vor-Writer-Grenze.

**Zu prüfen:** Writer wurde trotz fehlgeschlagenem Intent gerufen oder vorherige Daten sind unlesbar.

<a id="s001.b02"></a>
### S001.B02 — Zustellversuch erhalten, Empfang offen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Intent war dauerhaft, Writer oder Antwort liegt im Stromausfallfenster; Datenträger kehrt lesbar zurück.

**Warum bleibt oder endet der Zustand?** Unverändertes verbrauchtes Budget hält erneute verwaltete Zustellung an; neue Evidenz und eine nötige Entscheidung beenden nur diese Sperre.

**Zugeordnete Residues:** [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002)

**Was bleibt warum nutzbar?** Operator kann Versuch und Prompt inspizieren, aber aus ihnen keinen Empfang ableiten.

**Zu prüfen:** Retry mit unverändertem Budget ruft Writer erneut oder passende Empfangsevidenz ist bereits vorhanden.

<a id="s001.b03"></a>
### S001.B03 — Einzige benötigte lokale Fakten vernichtet

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Zusätzlich zum Stromausfall sind alle zugänglichen Kopien der benötigten Fakten irreparabel zerstört.

**Warum bleibt oder endet der Zustand?** Ohne unabhängige Rekonstruktionsquelle bleibt exakt diese Information verloren.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für exakte Rekonstruktion dieser Fakten bleibt keines; Stromausfall allein hätte diesen Verlust nicht bewiesen.

**Zu prüfen:** Eine lesbare unabhängige Kopie enthält die fehlenden Fakten.

**Architekturfolge für diesen Stressor:** Commit-Schnittpunkte und Promptempfang getrennt journalisieren. Ein Wiederanlauf darf weder aus fehlendem Resultat Nichtzustellung noch aus FULL-WAL physische Unzerstörbarkeit folgern.

<a id="s002"></a>
## S002 — Akku bläht sich auf und Gerät muss sofort außer Betrieb

**Ursprung:** operations; A6-Zustände: OP002, OP014, OP001, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s002.b01"></a>
### S002.B01 — Sicherheitsabschaltung mit erhaltenem Datenbestand

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Gerät sofort außer Betrieb, Datenträger bleibt intakt und später sicher lesbar.

**Warum bleibt oder endet der Zustand?** Bis sichere kompatible Hardware verfügbar ist keine lokale Ausführung.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Berechtigter Wiederhersteller kann später den Taskbestand nutzen; aktuelle Rechenleistung bleibt weg.

**Zu prüfen:** Taskdaten sind mit dem Gerät zerstört oder Ersatz kann sie nicht dekodieren.

<a id="s002.b02"></a>
### S002.B02 — Auf Ersatzgerät bleibt alter Versuch gesperrt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Sicher gelesener Datenträger enthält einen möglicherweise übergebenen Promptversuch.

**Warum bleibt oder endet der Zustand?** Hardwareersatz beseitigt nicht Empfangsungewissheit; diese endet nur durch zugeordnete Evidenz oder bewusste weitere Autorisierung.

**Zugeordnete Residues:** [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002)

**Was bleibt warum nutzbar?** Der erhaltene Versuch ist Entscheidungsgrundlage und keine Erlaubnis zum doppelten Start.

**Zu prüfen:** Ersatz startet denselben Versuch ohne Budgetprüfung.

<a id="s002.b03"></a>
### S002.B03 — Kein lesbarer Ersatzbestand

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Gerät darf nicht mehr betrieben werden, Zustand des Datenträgers und unabhängiger Kopien unbekannt.

**Warum bleibt oder endet der Zustand?** Ausgang bleibt bis sichere Bestandsklärung offen; nicht als Totalverlust umetikettieren.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Derzeit ist kein nutzbarer Datenzugang belegt, daher auch kein erfundener Export als Residue.

**Zu prüfen:** Sicherer Lesetest oder bestätigtes Kopieninventar entscheidet den Ausgang.

**Architekturfolge für diesen Stressor:** Hardwareverfügbarkeit von Datenerhalt und Zustellfreigabe trennen; Factory kann keine sichere Batterienutzung oder Ersatzhardware garantieren.

<a id="s003"></a>
## S003 — Thermal Throttling macht alle Deadline-Schätzungen falsch

**Ursprung:** operations; A6-Zustände: OP001, OP003, OP004. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s003.b01"></a>
### S003.B01 — Langsamere endliche Arbeit

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Dauerhafte begrenzte Originalqueue, endliche Last und keine Timeout-Reproduktion.

**Warum bleibt oder endet der Zustand?** Nach Abkühlung und bei Dienst über Zulauf wird verbleibende gültige Arbeit abgearbeitet.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Scheduler behält identifizierbare Originalaufträge trotz falscher Laufzeitschätzung.

**Zu prüfen:** Originalaufträge gehen verloren oder Queue wächst ohne Zulauf und ohne neue Produzenten.

<a id="s003.b02"></a>
### S003.B02 — Dauerzulauf über gedrosselter Kapazität

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Neue Aufträge übersteigen thermisch reduzierte Dienstleistung, Aufnahme begrenzt.

**Warum bleibt oder endet der Zustand?** Äußerer Zulauf erhält Verzögerung, Ablehnungen verhindern keine verpassten Fristen.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Zugelassene Originalarbeit bleibt planbar, nicht jede eingehende Arbeit und nicht jede Frist.

**Zu prüfen:** Zugelassene Queue wird unbeschränkt oder verliert ihre Original-IDs.

<a id="s003.b03"></a>
### S003.B03 — Timeouts halten ein Hochlastregime

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Rückstau erhöht Latenz, Timeouts erzeugen mehr Arbeit, Last senkt Nutzabschlüsse; ein begrenztes wiederkehrendes Hochlastregime besteht bei festem Grundbedarf nach Abkühlung.

**Warum bleibt oder endet der Zustand?** Rückstau → Timeouts → neue Last → weniger Abschlüsse → Rückstau. Unbegrenztes Wachstum wäre stattdessen Eskalation.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Solange Store zugänglich bleibt, kann Operator Originaltaskfakten lesen; Fortschritt und Deadlinequalität sind nicht erhalten.

**Zu prüfen:** Nach Entfernen der Hitze bei gleichem Grundbedarf und gleicher Policy kehren niedrige und hohe Startlast zum selben gesunden Verlauf zurück.

**Architekturfolge für diesen Stressor:** Deadline-Schätzung von Aufnahmebudget und Retry-Erzeugung trennen; Überlastschutz ersetzt keine verfügbare Rechenkapazität.

<a id="s004"></a>
## S004 — RAM-Fehler korrumpiert ein Ergebnis ohne Prozesscrash

**Ursprung:** operations; A6-Zustände: OP005, OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s004.b01"></a>
### S004.B01 — Falsche Bytes als Resultat akzeptiert

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** RAM-Fehler verändert relevante Ausgabe, Formvalidierung akzeptiert, kein unabhängiger sachlicher Prüfbezug vorhanden.

**Warum bleibt oder endet der Zustand?** Gespeicherter Fehler bleibt bis echte neue Evidenz eintrifft; kein notwendiger Regelkreis.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskkern belegt die Annahme, nicht Korrektheit des Ergebnisses; die betroffene Richtigkeitsgarantie fehlt.

**Zu prüfen:** Aufgabenbezogene Vergleichsdaten zeigen die Ausgabe war richtig oder vor Nutzung abgelehnt.

<a id="s004.b02"></a>
### S004.B02 — Unabhängiger Vergleich erkennt Abweichung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Unbetroffener Prüfbezug zur beschädigten Eigenschaft existiert und wird vor sachlicher Abnahme genutzt.

**Warum bleibt oder endet der Zustand?** Abnahme wartet auf überprüften Ersatz, nicht auf bloß erneute Zustimmung.

**Zugeordnete Residues:** [OPR034: Unabhängiger aufgabenbezogener Prüfbezug](residues.md#opr034)

**Was bleibt warum nutzbar?** Prüfer kann konkrete falsche Eigenschaft feststellen; kein universelles Fehlerorakel behauptet.

**Zu prüfen:** Gleich geformter korrumpierter Output wird trotz passendem Prüfbezug als richtig angenommen.

**Architekturfolge für diesen Stressor:** Fachliche Resultatprüfung braucht einen unabhängigen Prüfbezug; Checksummen und Completion-Formprüfung erkennen keine beliebige RAM-verursachte Bedeutungsänderung.

<a id="s005"></a>
## S005 — Einbruch entwendet den entsperrten einzigen Mac

**Ursprung:** operations; A6-Zustände: OP002, OP007, OP001, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s005.b01"></a>
### S005.B01 — Gestohlene Autorität bleibt wirksam

**Art:** extern-erzwungen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Entsperrter einziger Mac enthält nutzbare Autorität; kein unabhängiger Besitzerzugang ist belegt.

**Warum bleibt oder endet der Zustand?** Solange diese Autorität akzeptiert wird, sind weitere fremde Handlungen möglich; tatsächliche Taten und Dauer sind unbekannt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für vertrauenswürdige Betreiberkontrolle ist kein nutzbares Residue belegt. Kein Backup oder Widerrufskanal wird hinzuerfunden.

**Zu prüfen:** Ein unabhängig nutzbarer legitimer Zugang und erfolgreiche Autoritätsabgrenzung werden nachgewiesen.

<a id="s005.b02"></a>
### S005.B02 — Unabhängige Daten und Wiederzugang erlauben begrenzte Rettung

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Vor Diebstahl existierten separat lesbarer Export und legitimer unabhängiger Widerrufs-/Wiederzugangsweg.

**Warum bleibt oder endet der Zustand?** Nach Vertrauensneuaufbau kann enthaltene Arbeit weitergeführt werden, unbekannte Diebesaktionen müssen getrennt geklärt werden.

**Zugeordnete Residues:** [OPR008: Unabhängig lesbarer Export](residues.md#opr008), [OPR033: Unabhängiger legitimer Wiederzugang](residues.md#opr033)

**Was bleibt warum nutzbar?** Besitzer kann Exportfakten nutzen und Zugang zurückerlangen, aber erfolgte Offenlegung nicht rückgängig machen.

**Zu prüfen:** Export oder Recovery hängt tatsächlich am gestohlenen Mac oder kompromittierter Autorität.

<a id="s005.b03"></a>
### S005.B03 — Einzige erforderliche Information unerreichbar verloren

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Diebstahl führt dauerhaft zum Verlust aller Kopien einer benötigten Information und es gibt keine rechtmäßig erreichbare Rekonstruktionsquelle.

**Warum bleibt oder endet der Zustand?** Unter diesem geschlossenen Inventar keine exakte Rekonstruktion.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für diese Information keines; fremde weiter existierende Bytes bedeuten keinen nutzbaren Zugang für den Besitzer.

**Zu prüfen:** Gerät oder unabhängige vollständige Kopie wird rechtmäßig wieder verfügbar.

**Architekturfolge für diesen Stressor:** Diebstahl als getrennten Verlust von Besitzerzugang und Autoritätsvertrauen behandeln; Widerruf und Export nur bei tatsächlich unabhängigen Zugangswegen einplanen.

<a id="s006"></a>
## S006 — Brand vernichtet Gerät und daneben gelagerte Backupplatte

**Ursprung:** operations; A6-Zustände: OP006, OP002, OP011, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s006.b01"></a>
### S006.B01 — Gerät und einzige Sicherung vernichtet

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Beide genannten Medien enthalten alle Kopien des erforderlichen Inhalts, auch Schlüssel und sonstige Rekonstruktion fehlen.

**Warum bleibt oder endet der Zustand?** Der Brand endet, die Informationslücke bleibt ohne weitere Quelle bestehen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für den exakt verlorenen Inhalt keines; andere unabhängige Unternehmensfähigkeiten werden nicht mitvernichtet behauptet.

**Zu prüfen:** Inventur findet eine lesbare vollständige unabhängige Quelle.

<a id="s006.b02"></a>
### S006.B02 — Wirklich unabhängiger Export überlebt

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Abweichend vom Nur-zwei-Kopien-Inventar existiert schon vor Brand ein zugänglicher räumlich unabhängiger Export.

**Warum bleibt oder endet der Zustand?** Ausführung wartet auf Ersatz; enthaltene Fakten können sofort an anderem geeigneten Leser nutzbar sein.

**Zugeordnete Residues:** [OPR008: Unabhängig lesbarer Export](residues.md#opr008)

**Was bleibt warum nutzbar?** Berechtigte Wiederhersteller besitzen genau die exportierten Informationen, nicht automatisch den späteren Nachlauf.

**Zu prüfen:** Kopie teilt Brandbereich oder nötigen verlorenen Decoder-/Schlüsselzugang.

<a id="s006.b03"></a>
### S006.B03 — Unabhängige alte Sicherung hat Nachlauflücke

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Die überlebende Sicherung ist konsistent aber älter als mögliche Außenwirkungen.

**Warum bleibt oder endet der Zustand?** Lokale Präfixabfragen erzeugen keine fehlenden späteren Versuche.

**Zugeordnete Residues:** [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Präfix bleibt auswertbar; sichere Neuzustellung und Kenntnis späterer Effekte bleiben offen.

**Zu prüfen:** Verifiziertes Journal deckt den gesamten fehlenden Zeitraum ab.

**Architekturfolge für diesen Stressor:** Sicherungskopien nach gemeinsamem Schadensbereich bewerten. Kein Restoreverfahren kann aus zwei verbrannten einzigen Kopien fehlende Inhalte erzeugen.

<a id="s007"></a>
## S007 — Externe SSD trennt sich bei Scope-Kanonisierung

**Ursprung:** operations; A6-Zustände: OP008, OP026, OP025, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s007.b01"></a>
### S007.B01 — Abgetrennter Workspace verhindert Aufnahme

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** SSD enthält Workspace, Kernstore ist anderswo lesbar und Auflösung scheitert vor Reservation.

**Warum bleibt oder endet der Zustand?** Bis dieselbe Identität wieder sicher zugänglich ist bleibt diese Aufnahme aus.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Operator behält vorhandene Taskfakten; abgetrennte Scope-Dateien sind nicht verfügbar.

**Zu prüfen:** Fehlgeschlagene Auflösung führt trotzdem zu neuer wirksamer Reservation.

<a id="s007.b02"></a>
### S007.B02 — Gebundener Zugriff erkennt Verlust oder hält Identität

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Entwurf eines objektgebundenen Zugriffs ist tatsächlich auf diesem Pfad durchgesetzt; SSD trennt nach Prüfung.

**Warum bleibt oder endet der Zustand?** I/O scheitert oder erreicht nur das alte Objekt; ein neuer gleichnamiger Pfad wird nicht still verwendet.

**Zugeordnete Residues:** [OPR016: Objektgebundener Dateizugang](residues.md#opr016)

**Was bleibt warum nutzbar?** Factory-Schreiber behält die Begrenzung seines Zielobjekts, nicht garantierte Schreibverfügbarkeit.

**Zu prüfen:** Neu eingehängtes anderes Objekt empfängt den ursprünglich geprüften Schreibzugriff.

<a id="s007.b03"></a>
### S007.B03 — Pfad wird auf anderes Objekt umgedeutet

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur Pfad gespeichert, später neu aufgelöst und anderer Datenträger unter gleichem Namen angenommen.

**Warum bleibt oder endet der Zustand?** Falsche Zuordnung bleibt bis Identitätsklärung, kein autonomer Kreislauf nötig.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskabsicht kann im unbetroffenen Kern bleiben; Vertrauen in ausgeführte Zielzuordnung fehlt.

**Zu prüfen:** Persistierte Originalidentität wird vor jeder betroffenen Nutzung verglichen und Ersatz zurückgewiesen.

**Architekturfolge für diesen Stressor:** FileId-Reservierung nicht mit späterem sicheren Dateizugriff gleichsetzen. Factory-Schreiben muss am geprüften Objekt unter .factory bleiben, Datenträgerverlust darf weiter Fehler liefern.

<a id="s008"></a>
## S008 — macOS startet mitten in einer Freigabe ungefragt neu

**Ursprung:** operations; A6-Zustände: OP002, OP017, OP014, OP022, OP028. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s008.b01"></a>
### S008.B01 — Nicht committete Zustimmung fehlt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Neustart vor dauerhaftem Freigabeentscheid; Taskdaten bleiben lesbar und Annahmegrenze verlangt gültige Zustimmung.

**Warum bleibt oder endet der Zustand?** Gated Wirkung wartet auf tatsächlich neu geprüfte Entscheidung.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Auftrag ist lesbar, nicht die verlorene Zustimmung und nicht bereits erteilte Wirkungserlaubnis.

**Zu prüfen:** Wirkung wird aufgrund des verlorenen Klicks zugelassen.

<a id="s008.b02"></a>
### S008.B02 — Promptversuch überlebt Neustart ohne Empfangsnachweis

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Dauerhafter Versuch vor Neustart, keine passende Empfangsevidenz.

**Warum bleibt oder endet der Zustand?** Verbrauchtes Zustellbudget bleibt bis bewusster Entscheidung oder geklärtem Ergebnis bestehen.

**Zugeordnete Residues:** [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002)

**Was bleibt warum nutzbar?** Operator kann alten Versuch lesen; damit sind externe Wirkungen noch nicht geklärt.

**Zu prüfen:** Restart setzt das Budget zurück oder erzeugt einen zweiten Writer-Aufruf.

<a id="s008.b03"></a>
### S008.B03 — Teilnehmerannahme nach Restart ungewiss

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Außenwirkungsintent liegt vor, mögliche Annahme ist nicht dauerhaft lokal belegt.

**Warum bleibt oder endet der Zustand?** Warten endet durch verbindliche Teilnehmerquittung, nicht durch Pluginbereitschaft.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator hat genaue Operation zum Nachfragen, keine gesicherte Anzahl von Effekten.

**Zu prüfen:** Passende verbindliche Quittung liegt bereits vor oder Journal fehlt.

**Architekturfolge für diesen Stressor:** Freigabe-Commit, Promptversuch und Teilnehmerannahme sind drei unterschiedliche Restart-Schnittpunkte; ihre Freigaben dürfen nicht gemeinsam durch Neustart erneuert werden.

<a id="s009"></a>
## S009 — Hardware ersetzt Apple Silicon durch inkompatible Plattform

**Ursprung:** operations; A6-Zustände: OP002, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s009.b01"></a>
### S009.B01 — Runtime inkompatibel, Export interpretierbar

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorhandener Datenexport mit verfügbarem Decoder ist auf Ersatzplattform lesbar, Runtime nicht.

**Warum bleibt oder endet der Zustand?** Ausführung wartet auf kompatiblen Build oder Adapter, Lesen benötigt diese Runtime nicht.

**Zugeordnete Residues:** [OPR008: Unabhängig lesbarer Export](residues.md#opr008)

**Was bleibt warum nutzbar?** Berechtigter Bearbeiter kann enthaltene Fakten weiter verwenden und einen späteren Übergang planen.

**Zu prüfen:** Export erfordert genau die nicht lauffähige alte Runtime.

<a id="s009.b02"></a>
### S009.B02 — Portierbarkeit noch nicht geklärt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Keine Angaben zu Ersatz-OS, Decoderzugang oder adapterseitigen Abhängigkeiten.

**Warum bleibt oder endet der Zustand?** Bis konkreter Kompatibilitätstest keine eindeutige Nutzbarkeitsentscheidung.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Intakte Bits allein belegen weder nutzbaren Export noch totale Zerstörung.

**Zu prüfen:** Isolierter Lesetest und Build-/Adaptervertrag entscheiden die konkrete Plattform.

<a id="s009.b03"></a>
### S009.B03 — Kompatible Ausführung nach überprüftem Übergang

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Lesbarer Zustand und kompatibler Runtime-/Adapterbuild vorhanden, alte Autorität und Versuche vor Wiederanlauf geklärt.

**Warum bleibt oder endet der Zustand?** Endliche Übergangsarbeit endet bei erfolgreicher Abnahme, nicht automatisch bei Binary-Start.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Erhaltene Taskfakten können kontrolliert weiterverarbeitet werden.

**Zu prüfen:** Fehlende Decoder oder alte unklare Schreiber verhindern den behaupteten Übergang.

**Architekturfolge für diesen Stressor:** Portables Datenformat und portable Runtime separat qualifizieren. Exportlesbarkeit ist keine Zusage dass Herdr oder ein Harness auf Ersatzhardware läuft.

<a id="s010"></a>
## S010 — Wohnort verliert eine Woche Strom und Internet

**Ursprung:** operations; A6-Zustände: OP002, OP033, OP001, OP003, OP022. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s010.b01"></a>
### S010.B01 — Wohnort offline, Daten später lesbar

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Einzige ausführende Maschine ohne Strom, Datenträger intakt, keine aktive unabhängige Maschine.

**Warum bleibt oder endet der Zustand?** Woche ohne Strom hält Ausführung an; Datenzugriff kehrt erst mit sicherer Versorgung zurück.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Nach Wiederzugang sind alte Taskfakten nutzbar, während der Woche keine behauptete lokale Bedienbarkeit.

**Zu prüfen:** Datenträger verliert die Fakten oder unabhängige Maschine arbeitet bereits weiter.

<a id="s010.b02"></a>
### S010.B02 — Vorhandene unabhängige Instanz führt begrenzte Arbeit fort

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Bereits unabhängig versorgte Maschine mit benötigtem lesbarem Bestand und extern wirksam exklusiver Dispatchautorität existiert.

**Warum bleibt oder endet der Zustand?** Wohnortausfall verhindert dort nicht autorisierte Arbeit, Remote-Besitzerzugang kann dennoch fehlen.

**Zugeordnete Residues:** [OPR027: Exklusive Dispatch-Generation](residues.md#opr027), [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Autorisierter Dispatcher behält begrenzte Originalarbeit ohne Doppelautorität des ausgeschalteten Geräts.

**Zu prüfen:** Beide Instanzen können nach Rückkehr gleichzeitig mit kopierter Identität Wirkungen annehmen lassen.

<a id="s010.b03"></a>
### S010.B03 — Netzrückkehr klärt Altwirkungen nicht

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor Ausfall existiert ein Außenwirkungsintent ohne verbindlich erfassten Ausgang.

**Warum bleibt oder endet der Zustand?** Wieder vorhandenes Netz ermöglicht Abfrage, beendet Ungewissheit aber erst mit passender Evidenz.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Operator kann die konkrete offene Operation rekonzilieren statt pauschal nachholen.

**Zu prüfen:** Restart löst ungeklärte Operation blind als neue Wirkung aus.

**Architekturfolge für diesen Stressor:** Gemeinsame Strom-/Netzabhängigkeit offen ausweisen. Lokaler IPC ersetzt bei fehlendem Strom keinen Bedienpfad; Wiederanlauf braucht separate Altwirkungsprüfung.

<a id="s011"></a>
## S011 — SSD ist vor dem Intent-Commit voll

**Ursprung:** operations; A6-Zustände: OP008, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s011.b01"></a>
### S011.B01 — Neue Mutation ohne Platz abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** SSD voll vor Intent-Commit, bisherige Records bleiben lesbar; Aufruf benutzt C1.

**Warum bleibt oder endet der Zustand?** Ohne freie dauerhafte Kapazität keine neue Zustellung auf diesem Pfad.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Operator liest bestehende Tasks, während uncommitteter Versuch und neue Wirkung nicht behauptet werden.

**Zu prüfen:** Writer wird trotz fehlgeschlagenem Voraussetzungskommit aufgerufen.

<a id="s011.b02"></a>
### S011.B02 — Nach freiem Platz endlicher Wiederanlauf

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Kapazität ist wieder dauerhaft nutzbar, ursprünglicher Auftrag lesbar und weiter gültig.

**Warum bleibt oder endet der Zustand?** Neu validierter Versuch kann vor Writer committen; kein ungelöster früherer Versuch auf diesem Schnittpunkt.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Originalauftragsdaten vermeiden Neuanlage unter neuer Identität.

**Zu prüfen:** Vorheriger fehlgeschlagener Versuch hatte doch den Writer ausgelöst oder Auftrag ist inzwischen ungültig.

**Architekturfolge für diesen Stressor:** C1 Commitfehler-vor-Writer als enge Zustellgrenze erhalten. Lesefähigkeit und Reservekapazität gesondert prüfen, nicht aus disk-full ableiten dass alle Daten verloren seien.

<a id="s012"></a>
## S012 — Bitrot beschädigt einen alten kanonischen Event

**Ursprung:** operations; A6-Zustände: OP009, OP005, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s012.b01"></a>
### S012.B01 — Beschädigtes Event stoppt Replay

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Fehler ist erkennbar, unveränderte Rohbytes und verifizierbarer früherer Präfix bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Deterministischer Stop am Fehler bis gültiges Original oder Decoder vorliegt.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Diagnostiker kann intakte Eingaben prüfen und Fehlstelle lokalisieren, keine vollständige Rekonstruktion dahinter behaupten.

**Zu prüfen:** Replay überspringt die Stelle still oder Rohdaten werden zur Reparatur überschrieben.

<a id="s012.b02"></a>
### S012.B02 — Gültig geformtes Bitrot verändert Bedeutung

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Schaden ist syntaktisch gültig, kein unabhängiger ursprünglicher Wert für die betroffene Tatsache verfügbar.

**Warum bleibt oder endet der Zustand?** Falsche Reduktion kann gespeichert bleiben; Annahme gilt nicht als wahr.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für verlässliche Rekonstruktion der betroffenen Tatsache bleibt keines, auch wenn andere Events lesbar sind.

**Zu prüfen:** Eine unabhängige Originalquelle oder passender vorheriger Integritätsbezug identifiziert und rekonstruiert den Wert.

<a id="s012.b03"></a>
### S012.B03 — Einzige erforderliche Eventinformation zerstört

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Alle Kopien und Rekonstruktionsquellen des benötigten Eventinhalts fehlen dauerhaft.

**Warum bleibt oder endet der Zustand?** Unter geschlossenem Inventar kein exakter Replay dieser Tatsache möglich.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Kein Residue für genau diesen Rekonstruktionsschritt; nicht pauschal Verlust aller Tasks.

**Zu prüfen:** Vollständige unabhängige Eventkopie wird nachgewiesen.

**Architekturfolge für diesen Stressor:** Integritätsstopp und unveränderte Eingaben vor Replay-Erfolg priorisieren; valid aussehendes Bitrot verlangt unabhängigen Integritätsbezug und kann nicht rein aus neuer Prüfsumme entdeckt werden.

<a id="s013"></a>
## S013 — Projektionsmigration endet nach halber Tabellenänderung

**Ursprung:** operations; A6-Zustände: OP001, OP009. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s013.b01"></a>
### S013.B01 — Unterbrochene Migration rollt sauber zurück

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Managed Migration tatsächlich transaktional, SQLite-Recovery zuverlässig und alte Taskdaten lesbar.

**Warum bleibt oder endet der Zustand?** Rollback oder vollständiger Commit hinterlässt definiertes Schema; halbe Statementfolge ist keine halbe Commitfolge.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Vorhandene Taskdaten bleiben über das gültige Schema zugänglich; historische C9-Aussage ist kein getesteter Powercut.

**Zu prüfen:** Isolierter Abbruch des echten Managed-Pfads hinterlässt dauerhaft ein halbes Schema.

<a id="s013.b02"></a>
### S013.B02 — Halbe Projektion, kanonische Eingaben intakt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Nichttransaktionaler Projektionswechsel beschädigt Lesetabellen, vollständige Eventeingaben sind getrennt unverändert erhalten.

**Warum bleibt oder endet der Zustand?** Rebuild wartet auf geprüften Reduktor und darf keine Außenwirkung wiederholen.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Rebuilder kann aus Originaleingaben neu ableiten, nicht aus willkürlich geflickter halber Projektion.

**Zu prüfen:** Rebuild benötigt heutige externe Registry oder ruft ein Plugin auf.

<a id="s013.b03"></a>
### S013.B03 — Keine vollständige Rekonstruktionsquelle belegt

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Halbschema ist persistent und nur unbekannt vollständige Relationaldaten vorhanden.

**Warum bleibt oder endet der Zustand?** Bis Inventar und Konsistenz geprüft sind bleibt Rekonstruktionsfähigkeit offen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Kein imaginärer Eventstore als Rettung eines nur relationalen Bestands.

**Zu prüfen:** Konsistenter vollständiger Ausgangsbestand wird gefunden und deterministisch gelesen.

**Architekturfolge für diesen Stressor:** Transaktionalen Schemawechsel von späterer Projektionsrekonstruktion unterscheiden. Eine Projektion darf nur aus erhaltenen kanonischen Fakten neu entstehen, nie Plugins beim Replay ausführen.

<a id="s014"></a>
## S014 — WAL-Datei wird beim Dateikopieren vergessen

**Ursprung:** operations; A6-Zustände: OP011, OP009, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s014.b01"></a>
### S014.B01 — Konsistente Kopie ohne WAL hat unsichtbaren Nachlauf

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Kopie bleibt lesbar, benötigte spätere Commits lagen nur im ausgelassenen WAL.

**Warum bleibt oder endet der Zustand?** Präfixabfragen liefern den Nachlauf nie; neue unabhängige Evidenz erforderlich.

**Zugeordnete Residues:** [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Restore-Bearbeiter kann enthaltene frühere Fakten nutzen, nicht niemals-gesendet daraus folgern.

**Zu prüfen:** Checkpointbeleg zeigt dass alle benötigten Commits schon enthalten waren.

<a id="s014.b02"></a>
### S014.B02 — Checkpoint machte WAL für diesen Stand entbehrlich

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor konsistenter Kopie waren alle benötigten Commits checkpointed und keine spätere Wirkung fehlt.

**Warum bleibt oder endet der Zustand?** Validierte Öffnung erlaubt endlichen Restore ohne diese Nachlauflücke.

**Zugeordnete Residues:** [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Der nachgewiesen vollständige Sicherungsstand ist konkret nutzbar.

**Zu prüfen:** Integritätsprüfung oder unabhängige Effekthistorie zeigt fehlende Commits.

<a id="s014.b03"></a>
### S014.B03 — Kopie selbst inkonsistent

**Art:** halt. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Dateien zu widersprüchlichen Zeitpunkten kopiert, keine zuverlässig lesbaren erforderlichen Fakten nachgewiesen.

**Warum bleibt oder endet der Zustand?** Öffnung oder Replay stoppt bis gültige Quelle vorliegt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für zuverlässigen Restore ist kein konsistenter Präfix belegt; nicht automatisch alle Bits vernichtet.

**Zu prüfen:** Ein geprüfter konsistenter Ausgangsbestand lässt sich ohne Raten lesen.

**Architekturfolge für diesen Stressor:** Snapshot-Konsistenz und Nachlaufvollständigkeit getrennt belegen; VACUUM-INTO-Helfer C7 ist kein Beleg für Vollständigkeit einer manuellen Dateikopie.

<a id="s015"></a>
## S015 — Restore stellt eine Woche alte Tasks ohne spätere Versuche her

**Ursprung:** operations; A6-Zustände: OP011, OP023, OP001, OP028, OP014. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s015.b01"></a>
### S015.B01 — Alter Präfix mit unbekannter späterer Zustellung

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Sicherung zeigt queued ohne Versuch, spätere reale Versuche fehlen möglicherweise.

**Warum bleibt oder endet der Zustand?** C2-Rows allein lösen Lücke nicht; erneute Anfrage erzeugt keine historische Kenntnis.

**Zugeordnete Residues:** [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Alte Auftragsfakten sind nutzbar, sichere Neuzustellung ist nicht nachgewiesen.

**Zu prüfen:** Verifizierter Nachlauf oder Teilnehmerquittung klärt sämtliche betroffenen Versuche.

<a id="s015.b02"></a>
### S015.B02 — Restore bleibt kontrolliert nicht sendefähig

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alte Kopie hat tatsächlich durchgesetzte fehlende Dispatchautorität bis überprüfte Übernahme.

**Warum bleibt oder endet der Zustand?** Lesende Inventur geht weiter, Dispatch bleibt unabhängig vom queued-Label gesperrt.

**Zugeordnete Residues:** [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003), [OPR027: Exklusive Dispatch-Generation](residues.md#opr027)

**Was bleibt warum nutzbar?** Wiederhersteller kann alten Bestand prüfen ohne daraus automatisch neue externe Effekte zu erzeugen.

**Zu prüfen:** Kopierter Status oder lokales aktiv-Flag umgeht die Sendesperre.

<a id="s015.b03"></a>
### S015.B03 — Blind neu zugestellt und doppelt angenommen

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nach Snapshot ausgeführte Wirkung wird wegen fehlender Row erneut ausgelöst, Teilnehmer dedupliziert nicht.

**Warum bleibt oder endet der Zustand?** Doppelte historische Annahme bleibt, mögliche finanzielle Korrektur ist gesonderte neue Handlung.

**Zugeordnete Residues:** [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Präfix bleibt früheres Beweismaterial, garantiert aber keine Einmaligkeit oder Kenntnis des Gesamtzählers.

**Zu prüfen:** Teilnehmer belegt nur eine Annahme oder beide Bestellungen waren beabsichtigt.

**Architekturfolge für diesen Stressor:** Alten Snapshot zunächst nicht sendefähig machen und fehlenden Nachlauf ausweisen. C2 kann nur enthaltene Versuche sperren; Restore darf Abwesenheit einer Row nicht zur Außenwirkungserlaubnis machen.

<a id="s016"></a>
## S016 — Referenziertes Artefakt wurde manuell gelöscht

**Ursprung:** operations; A6-Zustände: OP012, OP001, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s016.b01"></a>
### S016.B01 — Nur Taskbezug bleibt nach Löschung

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Gelöschte Datei war einzige Quelle des Originalinhalts, Taskdatensatz bleibt lesbar.

**Warum bleibt oder endet der Zustand?** Pfadabfrage liefert fehlend, wiederholte Abfrage rekonstruiert keine Bytes.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Task-ID und Zusammenfassung können nachverfolgt werden; für exakte Artefaktbytes bleibt keines.

**Zu prüfen:** Eine unveränderte Originalkopie wird zugänglich.

<a id="s016.b02"></a>
### S016.B02 — Übernommenes Originalobjekt bleibt lesbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor Löschung des Arbeitsdateipfads wurde Original dauerhaft unveränderlich übernommen.

**Warum bleibt oder endet der Zustand?** Pfadverlust endet als Zugriffsproblem sobald Leser die gespeicherte Objektreferenz benutzt.

**Zugeordnete Residues:** [OPR005: Erhaltenes originales Artefaktobjekt](residues.md#opr005)

**Was bleibt warum nutzbar?** Ergebnisnutzer kann genau alte Bytes lesen, nicht nur einen alten Pfad anzeigen.

**Zu prüfen:** Objektreferenz löst auf geänderte Bytes oder fehlendes Objekt auf.

**Architekturfolge für diesen Stressor:** Artefaktpfad und tatsächlich übernommenes Objekt separat speichern; ein Pfadrest repariert keine manuell gelöschten einzigen Bytes.

<a id="s017"></a>
## S017 — Backup-Key ist verloren obwohl alle Backups intakt sind

**Ursprung:** operations; A6-Zustände: OP013, OP001, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s017.b01"></a>
### S017.B01 — Intakte verschlüsselte Backups derzeit nicht entschlüsselbar

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur bekannter Schlüssel verloren, weitere legitime Recoverywege sind noch ungeprüft.

**Warum bleibt oder endet der Zustand?** Bis Schlüsselzugang geklärt ist bleibt Klartextzugang aus; Bits können erhalten werden.

**Zugeordnete Residues:** [OPR007: Intaktes verschlüsseltes Sicherungsmaterial](residues.md#opr007)

**Was bleibt warum nutzbar?** Verwahrer behält Sicherungsmaterial, Besitzer hat dadurch noch keine Tasklesefähigkeit.

**Zu prüfen:** Ciphertext ist ebenfalls beschädigt oder unabhängiger Schlüssel funktioniert bereits.

<a id="s017.b02"></a>
### S017.B02 — Unabhängiger Schlüsselwiederzugang existiert wirklich

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Separater legitimer Schlüsselwiedergewinnungsweg und lesbare Backups sind vor Verlust eingerichtet und erreichbar.

**Warum bleibt oder endet der Zustand?** Nach verifizierter Wiedergewinnung kann Entschlüsselung erfolgen, Snapshotalter bleibt eigene Frage.

**Zugeordnete Residues:** [OPR007: Intaktes verschlüsseltes Sicherungsmaterial](residues.md#opr007), [OPR033: Unabhängiger legitimer Wiederzugang](residues.md#opr033)

**Was bleibt warum nutzbar?** Besitzer kann vorhandenes Material wieder nutzbar machen, nicht fehlenden Nachlauf erzeugen.

**Zu prüfen:** Recovery benötigt gerade den einzigen verlorenen Schlüssel.

<a id="s017.b03"></a>
### S017.B03 — Alle Entschlüsselungsinformationen unwiederbringlich weg

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Wirksame Verschlüsselung, keine Schlüsselanteile Klartextkopie oder innerhalb erforderlichem Horizont realistische Rekonstruktion vorhanden.

**Warum bleibt oder endet der Zustand?** Klartext bleibt unter diesem Inventar nicht rekonstruierbar.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für benötigten Klartext keines; intakter Ciphertext ist keine erhaltene Arbeitsfähigkeit.

**Zu prüfen:** Legitime alternative Entschlüsselung oder vollständige Klartextquelle wird nachgewiesen.

**Architekturfolge für diesen Stressor:** Ciphertextinventar getrennt von nutzbaren Schlüsseln und realistischen Recoveryfristen darstellen. Keine Secrets oder geheimen Recoveryanteile in Taskprotokolle aufnehmen.

<a id="s018"></a>
## S018 — Reduktor liest heutige Registry beim historischen Replay

**Ursprung:** operations; A6-Zustände: OP010, OP001, OP009. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s018.b01"></a>
### S018.B01 — Heutige Registry verändert historische Projektion

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Reduktor verwendet kausal relevante heutige Registrywerte; unveränderte Eventbytes bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Gleicher Log mit verschiedener Registry liefert verschiedene Ausgaben, kein Attraktor.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Diagnostiker kann Eingabebytes vergleichen; fehlende damalige Autorisierung lässt sich daraus nicht automatisch rekonstruieren.

**Zu prüfen:** Zwei verschiedene heutige Registries ergeben dieselbe korrekte Ausgabe für den behauptet abhängigen Schritt.

<a id="s018.b02"></a>
### S018.B02 — Historische Werte sind vollständig festgehalten

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Originalereignisse enthalten benötigte Registryrevision und Werte; korrigierter reiner Reduktor ist geprüft.

**Warum bleibt oder endet der Zustand?** Replay mit denselben historischen Eingaben endet reproduzierbar ohne Livezugriff.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004), [OPR026: Historische Scope-Identität mit Lebenszyklus](residues.md#opr026)

**Was bleibt warum nutzbar?** Historiennutzer kann damalige Zuordnung korrekt prüfen statt alte Rechte umzudeuten.

**Zu prüfen:** Replay greift noch auf aktuelle Registry zu oder historische Werte fehlen.

<a id="s018.b03"></a>
### S018.B03 — Damals relevante Tatsache nie aufgezeichnet

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Heutige Registry ist verändert und der damalige entscheidende Wert ist in keiner verfügbaren Quelle enthalten.

**Warum bleibt oder endet der Zustand?** Ohne unabhängige historische Quelle bleibt genaue Interpretation offen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für belegte damalige Autorisierung ist keine Restfähigkeit vorhanden; kein Wunschwert wird eingesetzt.

**Zu prüfen:** Eine zulässige authentische Quelle belegt den fehlenden damaligen Wert.

**Architekturfolge für diesen Stressor:** Registrywerte die historische Bedeutung steuern in kanonischen Fakten festhalten; reines Replay darf nicht von heutiger Registry abhängen.

<a id="s019"></a>
## S019 — Schema enthält neueren Payload-Typ als installiertes Binary

**Ursprung:** operations; A6-Zustände: OP009, OP001, OP005. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s019.b01"></a>
### S019.B01 — Alter Decoder stoppt an neuer Payload

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Strikter Decoder erkennt fehlende Unterstützung, Originalbytes sind weiter lesbar.

**Warum bleibt oder endet der Zustand?** Bis kompatible geprüfte Dekodierung verfügbar ist keine vollständige Projektion hinter dem Stop.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Replaybearbeiter kann neuen Typ und Originaldaten an einen passenden Decoder übergeben.

**Zu prüfen:** Alter Reader verwirft den Typ still oder verändert die Originalbytes.

<a id="s019.b02"></a>
### S019.B02 — Geprüfter kompatibler Decoder verarbeitet Original

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Decoder oder verlustfreier Upcaster mit erhaltener Semantik ist verfügbar.

**Warum bleibt oder endet der Zustand?** Endliche Rekonstruktion endet nach kompletter überprüfter Reduktion.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Erhaltene Eingaben tragen Wiederaufbau, nicht bloß eine höhere schema_version.

**Zu prüfen:** Vergleich mit spezifizierter neuer Semantik zeigt verlorene Bedeutung.

<a id="s019.b03"></a>
### S019.B03 — Stilles Ignorieren zerstört Vollständigkeitsbehauptung

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Reader meldet Erfolg obwohl relevante unbekannte Payload übersprungen wird, Originalbytes bleiben getrennt erhalten.

**Warum bleibt oder endet der Zustand?** Falsche Projektion bleibt bis Vergleich oder neuer Decoder.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Originale erlauben spätere Korrektur, aktueller Erfolgsstatus belegt keine korrekte Rekonstruktion.

**Zu prüfen:** Übersprungener Typ ist nachweislich irrelevant für alle behaupteten Projektionseigenschaften.

**Architekturfolge für diesen Stressor:** Payloaddecoder-Version vom relationalen Schema trennen. Nicht verstandene Events müssen als erhaltene Eingaben stoppbar bleiben statt als ignorierte Änderungen zu verschwinden.

<a id="s020"></a>
## S020 — Backup-Retention löscht die einzige Kopie benötigter Inhalte

**Ursprung:** operations; A6-Zustände: OP012, OP006, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s020.b01"></a>
### S020.B01 — Einzige noch benötigte Inhalte gelöscht

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Retention entfernt letzte zugängliche Quelle der benötigten Bytes, kein vollständiger anderer Rekonstruktionsweg.

**Warum bleibt oder endet der Zustand?** Löschung endet, fehlende Information bleibt unter geschlossenem Inventar weg.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für diese Inhalte keines; verbleibender Pfad und Retentionsregel erzeugen sie nicht.

**Zu prüfen:** Inventur findet tatsächlich lesbares Original oder weist fehlenden Bedarf nach.

<a id="s020.b02"></a>
### S020.B02 — Unabhängig gehaltenes Original bleibt nutzbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Retention betrifft eine Sicherung, notwendiges unveränderliches Artefaktobjekt existiert separat und zugänglich.

**Warum bleibt oder endet der Zustand?** Referenz löst auf dieses überlebende Objekt, die gelöschte Sicherung wird nicht benötigt.

**Zugeordnete Residues:** [OPR005: Erhaltenes originales Artefaktobjekt](residues.md#opr005)

**Was bleibt warum nutzbar?** Ergebnisnutzer kann benötigte Originalbytes weiter lesen.

**Zu prüfen:** Aufbewahrtes Objekt war ebenfalls Teil derselben Löschmenge.

<a id="s020.b03"></a>
### S020.B03 — Gelöschtes Material war nicht mehr erforderlich

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Nachweisbarer Abhängigkeitsstand zeigt keinen verbleibenden Task- oder Replaybedarf, benötigte Taskfakten bleiben lesbar.

**Warum bleibt oder endet der Zustand?** Entfallene Inhalte erzeugen keinen offenen Wiederherstellungsauftrag.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Noch benötigte Taskfakten bleiben nützlich; keine Aussage über unnötige verlorene Bytes.

**Zu prüfen:** Später belegte bestehende Referenz benötigt genau die gelöschte Information.

**Architekturfolge für diesen Stressor:** Retention muss Referenzbedarf und wirklich aufbewahrte Objekte kennen. Der vorhandene Backuphelper löscht nicht selbst Retentioninhalte und ist kein Beleg für deren Erhaltung.

<a id="s021"></a>
## S021 — Sommerzeit wiederholt dieselbe lokale Cron-Minute

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s021.b01"></a>
### S021.B01 — Eine Faltminute meint einen Auftrag

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Zeitauftrag verlangt einmal je lokalem Datums-/Minutenlabel, dauerhaftes Ledger dedupliziert beide Fold-Emissionen.

**Warum bleibt oder endet der Zustand?** Zweite Emission findet dieselbe Run-Zuordnung; nur Terminaufnahme abgeschlossen, nicht automatisch Taskarbeit.

**Zugeordnete Residues:** [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011), [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Dispatcher kann erklären warum beide Uhranzeigen denselben Auftrag meinen.

**Zu prüfen:** Zweite Fold-Emission erzeugt einen zweiten Run oder Auftrag verlangte ausdrücklich beide Instanten.

<a id="s021.b02"></a>
### S021.B02 — Beide physische Termine sind gewollt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Vertrag verlangt beide UTC-Instanten, Fold beziehungsweise Offset ist Teil stabiler Occurrence-ID.

**Warum bleibt oder endet der Zustand?** Je physischem Termin eine Zuordnung, erneute Lieferung desselben Termins bleibt idempotent.

**Zugeordnete Residues:** [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011), [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Zwei Runs sind hier erhaltene Absicht statt Doppelschaden; spätere Geschäftswirkung braucht eigene Zustimmung.

**Zu prüfen:** Retry desselben Instants erzeugt weiteren Run oder beide Instanten kollabieren entgegen dem Vertrag.

<a id="s021.b03"></a>
### S021.B03 — Bedeutung der doppelten Minute fehlt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Weder gespeicherte Absicht noch Fold-ID-Regel sind festgelegt.

**Warum bleibt oder endet der Zustand?** Die Uhrumstellung endet, richtige Runzahl bleibt ohne Entscheidung unbestimmbar.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für nachweislich richtige Terminaufnahme ist kein bestimmter Gegenstand belegt; kein universelles einmal oder zweimal.

**Zu prüfen:** Expliziter Auftrag und isolierter Fold-Test legen die gewünschte Zuordnung fest.

**Architekturfolge für diesen Stressor:** Fold-Vertrag zuerst festlegen: entweder ein zivil beschrifteter Termin oder zwei verschiedene physische Termine. Occurrence-Key muss genau diesen Vertrag ausdrücken und vor Triggerbestätigung dauerhaft dem Run zugeordnet sein.

<a id="s022"></a>
## S022 — Sommerzeit überspringt geplanten Zeitpunkt

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s022.b01"></a>
### S022.B01 — Nicht existente Minute wird ausdrücklich ausgelassen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag vereinbart Skip bei Gap, Ledger erfasst die Auslassung statt einen Run zu erfinden.

**Warum bleibt oder endet der Zustand?** Terminverwaltung ist abgeschlossen, die nicht verlangte Ersatzarbeit wird nicht erzeugt.

**Zugeordnete Residues:** [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011), [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Operator kann sehen warum kein Run existiert; kein erledigter Geschäftsauftrag wird daraus behauptet.

**Zu prüfen:** Auslassung wird als ausgeführte Arbeit angezeigt oder Auftrag verlangte Ersatztermin.

<a id="s022.b02"></a>
### S022.B02 — Vertraglich nachzuholender Termin bleibt identifizierbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag verlangt nächsten gültigen Zeitpunkt, verschobene Occurrence behält stabile Identität und Aufgabe bleibt gültig.

**Warum bleibt oder endet der Zustand?** Begrenzte Nachholarbeit endet nach Annahme und Abarbeitung; neue externe Zustimmung bleibt gesondert.

**Zugeordnete Residues:** [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011), [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Dispatcher kann den fehlenden zivilen Termin seinem Ersatz zuordnen ohne zweite Absicht zu erzeugen.

**Zu prüfen:** Nachholen verliert Herkunft oder führt abgelaufene Aufgabe ungeprüft aus.

<a id="s022.b03"></a>
### S022.B03 — Keine Regel für ausgelassene Zeit

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Unbekannt ob lokaler Termin Dauerintervall oder Pflicht bis Tagesende gemeint ist.

**Warum bleibt oder endet der Zustand?** Ohne Vertragsentscheidung ist Fehlen weder korrekt noch automatisch Schaden.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Kein nachgewiesener Termin-Erhalt durch bloße Cronzeichenfolge.

**Zu prüfen:** Ein dokumentierter Geschäftsauftrag entscheidet Skip gegen Nachholen.

**Architekturfolge für diesen Stressor:** Gap-Disposition mit beabsichtigter Geschäftsbedeutung speichern: auslassen ist nicht erfüllen, Nachholen verlangt Gültigkeitsprüfung und gibt keine externe Sendefreigabe.

<a id="s023"></a>
## S023 — NTP setzt Uhr zwölf Stunden zurück

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s023.b01"></a>
### S023.B01 — Rücksprung ändert Anzeigen nicht die Occurrence-Identität

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Aufträge und bereits materialisierte Occurrences sind dauerhaft erfasst; Retry derselben Zeitabsicht behält Key.

**Warum bleibt oder endet der Zustand?** Rückgestellte Wandzeit erzeugt keine zweite Zuordnung; künftige Terminzeit benötigt weiter definierten Zeitvertrag.

**Zugeordnete Residues:** [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011), [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Scheduler behält nachvollziehbare Absicht und bereits getroffene Terminentscheidungen.

**Zu prüfen:** Gleicher logischer Termin erzeugt nach zwölfstündigem Rücksprung eine zusätzliche Run-ID.

<a id="s023.b02"></a>
### S023.B02 — Fristentscheidung wartet auf verlässliche Zeit

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Zeitauftrag verlangt absolute Ablaufzeit, Rücksprung erkannt und keine vertrauenswürdige aktuelle Zeit verfügbar.

**Warum bleibt oder endet der Zustand?** Bis verlässlicher Zeitabgleich oder neue rechtmäßige Entscheidung keine Aussage noch gültig.

**Zugeordnete Residues:** [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Prüfer kennt die beabsichtigte Frist und den fehlenden Vergleichswert, nicht deren aktuellen Wahrheitswert.

**Zu prüfen:** System akzeptiert neue Wirkung allein weil zurückgestellte Uhr wieder vor Expiry liegt.

<a id="s023.b03"></a>
### S023.B03 — Rücksprung wird ungeprüft als neue Gültigkeit benutzt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Wandzeit allein entscheidet Gültigkeit, bereits abgelaufene Wirkung wird tatsächlich später akzeptiert.

**Warum bleibt oder endet der Zustand?** Einmaliger Consent-Verstoß bleibt historische Tatsache, kein Attraktor aus Uhrfehler allein.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Unbetroffene Taskfakten können erhalten sein; korrekte zeitliche Zustimmung ist für die Wirkung nicht erhalten.

**Zu prüfen:** Annahme erfüllt trotz Uhrsprung den tatsächlichen Freigabevertrag.

**Architekturfolge für diesen Stressor:** Clock-Domänen je Frist und Schedule benennen. Monotone lokale Reihenfolge ersetzt keine vertrauenswürdige Wandzeit für externe Freigabeexpiry.

<a id="s024"></a>
## S024 — Laptop erwacht nach sechs Wochen mit tausenden fälligen Runs

**Ursprung:** operations; A6-Zustände: OP001, OP003, OP004. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s024.b01"></a>
### S024.B01 — Begrenzte Nachholarbeit wird abgearbeitet

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Endlicher gültiger Rückstand und Dienst über aktuellem Zulauf, stabile Aufnahme ohne Timeout-Neuschöpfung.

**Warum bleibt oder endet der Zustand?** Rückstand sinkt bis verbleibende Arbeit erledigt ist.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010), [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011)

**Was bleibt warum nutzbar?** Scheduler kann erhaltene Termine Originalaufträgen zuordnen und begrenzt abarbeiten.

**Zu prüfen:** Nachholung erzeugt neue Identitäten für schon aufgenommene Termine.

<a id="s024.b02"></a>
### S024.B02 — Fortgesetzter Zulauf hält Rückstand

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Ankommende gültige Arbeit übersteigt Dienst nach Wake, Aufnahmebudget bewahrt nur zugelassene Menge.

**Warum bleibt oder endet der Zustand?** Äußere Last erhält Warten; Rückstand allein ist keine Eigendynamik.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Bereits zugelassene Arbeit bleibt nutzbar, nicht alle Fristen oder alle abgewiesenen Eingänge.

**Zu prüfen:** Queue überschreitet Budget oder verliert zugelassene Originalaufträge.

<a id="s024.b03"></a>
### S024.B03 — Nachholwelle löst dauernde Retry-Reproduktion aus

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Bei unverändertem Grundbedarf führt hohe Queue zu Timeouts, diese erzeugen Last und hemmen Abschluss; begrenztes Hochlastregime bleibt nach ursprünglicher Wake-Welle.

**Warum bleibt oder endet der Zustand?** Queue → Timeout → Retrylast → weniger Nutzabschluss → Queue. Ohne begrenzte Wiederkehr nur Eskalation.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Lesbarer Taskkern bleibt nur solange Speicher und Zugriffsressourcen nicht ebenfalls erschöpfen.

**Zu prüfen:** Bei festem Grundbedarf und gleicher Policy kehren verschiedene Startqueues nach Welle zum selben gesunden Zustand zurück.

**Architekturfolge für diesen Stressor:** Nachholinventar mit bounded admission und Einzelgültigkeit verbinden. Eine sechs Wochen alte Terminliste darf weder pauschal gelöscht noch pauschal zur Sendefreigabe werden.

<a id="s025"></a>
## S025 — Zeitzonenregeln ändern sich nach Schedule-Erstellung

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s025.b01"></a>
### S025.B01 — Gepinnte Zeitregeln bleiben maßgeblich

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag verlangt gespeicherte tzdb-Version und diese ist weiterhin verfügbar.

**Warum bleibt oder endet der Zustand?** Regelupdate ändert diesen Auftrag nicht, zukünftige Materialisierung benutzt alte definierte Regeln.

**Zugeordnete Residues:** [OPR012: Expliziter Zeitauftrag](residues.md#opr012), [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011)

**Was bleibt warum nutzbar?** Dispatcher kann ursprüngliche Zeitabsicht wiedergeben und Wiederholungen zuordnen.

**Zu prüfen:** Gleicher Auftrag erzeugt nach Softwareupdate ungefragt andere physische Termine.

<a id="s025.b02"></a>
### S025.B02 — Aktuelle Zivilzeit gilt nach protokollierter Änderung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag verlangt aktuelle Regeln, Änderung mit Cutover und Behandlung bereits materialisierter Termine ist explizit festgelegt.

**Warum bleibt oder endet der Zustand?** Neue Revision steuert nur dafür bestimmte Termine; alte Zuordnungen bleiben erklärbar.

**Zugeordnete Residues:** [OPR012: Expliziter Zeitauftrag](residues.md#opr012), [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011)

**Was bleibt warum nutzbar?** Operator kann verschobene Termine auf eine legitime Regelentscheidung zurückführen.

**Zu prüfen:** Bereits zugestellter Termin wird still unter neuer Identität erneut aufgenommen.

<a id="s025.b03"></a>
### S025.B03 — Beabsichtigte Regelbindung unbekannt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Nur Zonenname gespeichert, keine Absicht zur Regeländerung oder Revisionserhaltung.

**Warum bleibt oder endet der Zustand?** Ohne zusätzliche Geschäftsentscheidung ist richtige Verschiebung nicht ableitbar.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für originalgetreue zukünftige Terminbedeutung kein belegter Prüfgegenstand.

**Zu prüfen:** Authentischer Auftrag benennt alte oder aktuelle Regelbindung und Cutover.

**Architekturfolge für diesen Stressor:** Regelversion und Änderungsentscheidung speichern, materialisierte Occurrences nicht still neu identifizieren. Zivilrechtlich aktuelle Regeln und gepinnte Regeln sind unterschiedliche erlaubte Aufträge.

<a id="s026"></a>
## S026 — Zwei Teilnehmer bestätigen Wirkungen mit widersprüchlichen Uhrzeiten

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s026.b01"></a>
### S026.B01 — Kausale Quittung ordnet abhängige Effekte

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide verbindlichen Quittungen überleben, zweite nennt geprüfte Vorgängerversion der ersten.

**Warum bleibt oder endet der Zustand?** Widersprüchliche Wandzeiten ändern belegte Kausalrelation nicht; Ordnungsfrage ist geklärt.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann genau diese Abhängigkeit feststellen, nicht alle globalen Effekte total ordnen.

**Zu prüfen:** Vorgängerverweis ist unverbindlich oder Teilnehmer akzeptierte ohne die referenzierte Version.

<a id="s026.b02"></a>
### S026.B02 — Effekte sind unabhängig und dürfen gleichzeitig sein

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Verbindliche Einzelquittungen vorhanden, Geschäftsvertrag benötigt keine Reihenfolge zwischen ihnen.

**Warum bleibt oder endet der Zustand?** Annahme beider Effekte ist geklärt, ungeklärte Wandzeitordnung beeinflusst diese Abnahme nicht.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Beide konkreten Wirkungen bleiben belegbar ohne künstliche zeitliche Gesamtreihenfolge.

**Zu prüfen:** Eine fachlich relevante Vorbedingung verlangt doch das Vorliegen des anderen Effekts.

<a id="s026.b03"></a>
### S026.B03 — Benötigte Reihenfolge unbeweisbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Geschäftsinvariante verlangt Reihenfolge, nur widersprüchliche Zeitstempel ohne vertrauenswürdige Kausalbelege vorhanden.

**Warum bleibt oder endet der Zustand?** Weitere lokale Abfragen derselben Stempel liefern keinen Ordnungsbeweis.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für verlässlichen Nachweis der benötigten Reihenfolge keines; Annahmen einzelner Effekte können trotzdem wahr sein.

**Zu prüfen:** Zugängliche verbindliche Versions- oder Vorgängerbelege entscheiden die Reihenfolge.

**Architekturfolge für diesen Stressor:** Quittungskausalität nur für tatsächlich belegte Abhängigkeiten nutzen. Lokaler Cursor und fremde Wandzeiten dürfen kein fehlendes verteiltes Vorher-Nachher erfinden.

<a id="s027"></a>
## S027 — Freigabe läuft während langem Plugin-Start ab

**Ursprung:** operations; A6-Zustände: OP017, OP027, OP028, OP022. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s027.b01"></a>
### S027.B01 — Abgelaufene Zustimmung blockiert neue Annahme

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Freigabeobjekt erhalten, tatsächliche Akzeptanzgrenze kann Ablauf vor Annahme zuverlässig prüfen.

**Warum bleibt oder endet der Zustand?** Ohne gültige erneute Entscheidung bleibt genau diese Wirkung gesperrt.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Entscheider kann die alte begrenzte Zustimmung und neue benötigte Entscheidung prüfen.

**Zu prüfen:** Teilnehmer nimmt nach vertraglichem Ablauf unter alter Freigabe trotzdem an.

<a id="s027.b02"></a>
### S027.B02 — Annahme lag verbindlich innerhalb Gültigkeit

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Quittung belegt irreversible Annahme vor Ablauf, nur Startup-/Antwortverarbeitung endet später.

**Warum bleibt oder endet der Zustand?** Späte lokale Antwort macht bereits legitim erfolgte Annahme nicht ungültig.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021), [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Operator kann genau akzeptierte Wirkung und damalige Zustimmung verbinden.

**Zu prüfen:** Quittung belegt nur Empfang oder Startup und nicht den erforderlichen Annahmemoment.

<a id="s027.b03"></a>
### S027.B03 — Zeitpunkt oder Annahme bleibt unbekannt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Lokales Intentjournal erhalten, entfernte Annahmezeit nicht verlässlich belegt.

**Warum bleibt oder endet der Zustand?** Warten auf verbindliche Evidenz, erneuter Start verlängert alte Zustimmung nicht.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020), [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Absicht und Freigabegrenze bleiben lesbar, erfüllte zeitliche Zustimmung bleibt offen.

**Zu prüfen:** Verbindlicher Annahmebeleg klärt den vertraglichen Zeitpunkt.

<a id="s027.b04"></a>
### S027.B04 — Annahme erfolgte nach Ablauf unter alter Zustimmung

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Freigabe wurde nur vor langem Pluginstart geprüft, Teilnehmer nimmt nach dem tatsächlich bindenden Ablauf an, ursprüngliches Freigabeobjekt bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Die verletzte zeitliche Zustimmung bleibt historische Eigenschaft. Späte neue Zustimmung oder Pluginbereitschaft autorisiert die alte Annahme nicht rückwirkend.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Entscheider kann die ursprüngliche Zeitgrenze untersuchen, nicht verlorene Zustimmungstreue oder eine genaue Annahmezeit aus dem Reviewobjekt allein herstellen.

**Zu prüfen:** Verbindliche Annahmeevidenz liegt innerhalb des tatsächlich erlaubten Zeitfensters oder die wirksame Annahmegrenze hat abgelaufene Wirkung verhindert.

**Architekturfolge für diesen Stressor:** Freigabevertrag muss Annahmemoment oder wirksame Teilnehmerprecondition benennen. Prüfung vor langem Pluginstart allein garantiert keine Zustimmung bei späterer Annahme.

<a id="s028"></a>
## S028 — Ein Task verarbeitet einen inzwischen abgelaufenen Preis

**Ursprung:** operations; A6-Zustände: OP005, OP027, OP017, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s028.b01"></a>
### S028.B01 — Abgelaufener Preis bleibt gültiger historischer Berichtswert

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag verlangt ausdrücklich Preis zum früheren Stichtag und erhaltener Prüfbezug belegt ihn.

**Warum bleibt oder endet der Zustand?** Aktuelle Gültigkeit ist für diese historische Aussage irrelevant.

**Zugeordnete Residues:** [OPR034: Unabhängiger aufgabenbezogener Prüfbezug](residues.md#opr034)

**Was bleibt warum nutzbar?** Prüfer kann den verlangten historischen Wert nutzen statt unnötig aktuelle Preise einzusetzen.

**Zu prüfen:** Auftrag verlangt aktuellen Kaufpreis oder historische Herkunft stimmt nicht.

<a id="s028.b02"></a>
### S028.B02 — Aktueller Kauf wartet auf neue Preisentscheidung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Altes geprüftes Freigabeobjekt enthält abgelaufenen Preis und Annahmegrenze prüft aktuelle Terms.

**Warum bleibt oder endet der Zustand?** Bis belegter neuer Preis und nötige Zustimmung vorhanden sind bleibt Kauf aus.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Operator kann alten Entscheidungsgegenstand abgrenzen, nicht neuen Preis aus ihm erzeugen.

**Zu prüfen:** Kauf wird mit ungenehmigten aktuellen Terms angenommen.

<a id="s028.b03"></a>
### S028.B03 — Veralteter Preis wird als aktuell akzeptiert

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Keine passende unabhängige Preisprüfung, Ergebnis oder externe Bestellung benutzt abgelaufenen Wert als gültig.

**Warum bleibt oder endet der Zustand?** Fehler bleibt bis neue sachliche Evidenz, keine notwendige Rückkopplung.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskdaten belegen was behauptet wurde, nicht aktuelle Preiskorrektheit.

**Zu prüfen:** Vertrag garantiert den alten Preis noch oder Ausgabe bezeichnet ihn korrekt als historisch.

**Architekturfolge für diesen Stressor:** Preisverwendung als historische Aussage oder aktuelle Transaktion unterscheiden. Frischeprüfung benötigt echte Preisquelle beziehungsweise garantierte Terms, nicht Resultatformvalidierung.

<a id="s029"></a>
## S029 — Maschine geht während monotonic-basierter Frist in Sleep

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s029.b01"></a>
### S029.B01 — Suspend zählt zur vertraglichen Dauer

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Gespeicherter Zeitauftrag verlangt inklusive Sleep verstrichene Dauer, verfügbares Primitive erfasst sie auch über erforderliche Übergänge.

**Warum bleibt oder endet der Zustand?** Nach Wake ist Frist gegebenenfalls sofort abgelaufen, nachgelagerte Handlung bleibt autorisierungspflichtig.

**Zugeordnete Residues:** [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Fristprüfer kann Restdauer gemäß ursprünglichem Auftrag berechnen.

**Zu prüfen:** Primitive pausiert in Sleep oder Restart verliert die erforderliche Zeitbasis.

<a id="s029.b02"></a>
### S029.B02 — Nur aktive Laufzeit zählt

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag verlangt aktive Bearbeitungsdauer und tatsächliches Primitive pausiert verlässlich im Sleep.

**Warum bleibt oder endet der Zustand?** Nach Wake läuft verbliebene aktive Dauer weiter; keine Aussage zu Wandzeitfreigaben.

**Zugeordnete Residues:** [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Timer erhält genau vereinbarte aktive Restzeit.

**Zu prüfen:** Geschäftliche Deadline ist realzeitlich oder Primitive zählt Sleep entgegen Vertrag.

<a id="s029.b03"></a>
### S029.B03 — Suspend- oder Restart-Zeitbasis fehlt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Fristbedeutung ist bekannt und gespeichert, aber tatsächlich verstrichene benötigte Dauer ist nicht rekonstruierbar.

**Warum bleibt oder endet der Zustand?** Bis belastbarer Zeitbezug oder explizite neue Disposition bleibt Ablaufentscheidung offen.

**Zugeordnete Residues:** [OPR012: Expliziter Zeitauftrag](residues.md#opr012)

**Was bleibt warum nutzbar?** Beabsichtigte Frist bleibt nutzbarer Prüfauftrag, ihr aktueller Ablauf nicht.

**Zu prüfen:** Plattformtrace oder persistenter verlässlicher Zeitanker rekonstruiert die Dauer.

**Architekturfolge für diesen Stressor:** Tatsächliches Timerprimitive und Sleep-Semantik spezifizieren. Aktive Laufzeit und reale verstrichene Dauer sind verschiedene Aufträge; monotonic allein ist keine davon.

<a id="s030"></a>
## S030 — Cron-Plugin feuert denselben Trigger nach jedem Neustart erneut

**Ursprung:** operations; A6-Zustände: OP026, OP001, OP023, OP003. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s030.b01"></a>
### S030.B01 — Restart liefert dieselbe Occurrence erneut

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Stabiler vertraglicher Key und atomare erhaltene Run-Zuordnung vor Triggerbestätigung.

**Warum bleibt oder endet der Zustand?** Jede Wiederholung liest dieselbe Zuordnung, Aufnahme endet einmal.

**Zugeordnete Residues:** [OPR011: Dauerhafte Schedule-Occurrence-Zuordnung](residues.md#opr011)

**Was bleibt warum nutzbar?** Dispatcher kann Originalrun weiterverfolgen ohne neue Aufgabe.

**Zu prüfen:** Neustart erzeugt bei gleichem Key einen zweiten Run.

<a id="s030.b02"></a>
### S030.B02 — Jeder Neustart erzeugt neue logische Aufgabe

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Key ändert sich je Restart und wiederholte externe Neustarts liefern laufend Arbeit.

**Warum bleibt oder endet der Zustand?** Restartzufuhr hält Zusatzlast; ohne weitere Neustarts ist sie endlich, kein autonomer Attraktor.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Vorhandene Taskfakten bleiben nachprüfbar, einmalige Terminaufnahme ist verloren.

**Zu prüfen:** Alle Neustartemissionen werden auf eine erhaltene Occurrence zurückgeführt.

<a id="s030.b03"></a>
### S030.B03 — Zusatzruns bewirken echte Doppelannahmen

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Neue Runs wurden jeweils effektfähig autorisiert oder ungeprüft versendet und Teilnehmer nimmt dasselbe beabsichtigte Geschäft mehrfach an.

**Warum bleibt oder endet der Zustand?** Historische Mehrfachannahmen bleiben, ohne weitere Wiederholungen kein Loop.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Erhaltene einzelne Intentjournale helfen untersuchen, garantieren aber keine fachliche Einmaligkeit.

**Zu prüfen:** Mehrere Runs führen nachweislich nur zu einer Annahme oder zu gewollt mehreren Geschäften.

**Architekturfolge für diesen Stressor:** Pluginneustart muss dieselbe dauerhaft materialisierte Occurrence wiederverwenden; mehrfacher Trigger ist nicht automatisch mehrfacher Geschäftseffekt.

<a id="s031"></a>
## S031 — Herdr ist unerreichbar während der Harness weiter Dateien schreibt

**Ursprung:** operations; A6-Zustände: OP015, OP001, OP016. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s031.b01"></a>
### S031.B01 — Harness schreibt weiter, verwaltete Reservation bleibt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Herdr unerreichbar, alter Harness hat weiterhin Schreibrechte, disconnected/restore-Pfad behält Lease.

**Warum bleibt oder endet der Zustand?** Zweiter verwalteter Start wartet, tatsächliche alte Arbeit kann weiterlaufen.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Startentscheider besitzt Beleg der Belegung und Operator später Taskfakten, nicht aktuelle Herdr-Liveness.

**Zu prüfen:** Normaler zweiter Start im selben stabilen Workspace wird trotz Lease zugelassen.

<a id="s031.b02"></a>
### S031.B02 — Beobachterverlust löst überlappende Schreiber aus

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Jemand gibt Lease vorschnell frei und startet Ersatz während alter Harness noch schreibt.

**Warum bleibt oder endet der Zustand?** Konflikt besteht solange beide aktiv schreiben, kein notwendiger selbsttragender Reparaturkreis.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Unbetroffener Taskkern bleibt nutzbar, exklusive Workspaceattribution nicht.

**Zu prüfen:** Alle alten Schreibrechte waren vor Ersatzstart nachweislich erloschen.

**Architekturfolge für diesen Stressor:** Beobachterausfall nicht als Writer-Tod behandeln. Restore-Lease und Reconnect-Presumption getrennt führen; produktweite Ausschlussgarantie verlangt alle Startpfade und Schreiber.

<a id="s032"></a>
## S032 — Pane-ID wird nach Neustart einer anderen Session gegeben

**Ursprung:** operations; A6-Zustände: OP015, OP025, OP016. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s032.b01"></a>
### S032.B01 — Anderer Sessionbezug wird ausgesondert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Neue Pane hat andere UUID, verbindliche Beobachtungskorrelation erkennt Diskrepanz und Reservation bleibt.

**Warum bleibt oder endet der Zustand?** Bis echter alter Writer geklärt ist keine Wiederverwendung allein wegen neuer Pane.

**Zugeordnete Residues:** [OPR014: Korrelierte Beobachtung mit Frischegrenze](residues.md#opr014), [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Operator kann Fremdbeobachtung von alter Belegung trennen.

**Zu prüfen:** Fremde UUID wird auf alte Session promoviert oder Lease aus Pane-Reuse gelöscht.

<a id="s032.b02"></a>
### S032.B02 — Fehlende UUID erlaubt falsche Zuordnung

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** UUID fehlt auf einer Seite, historisches C4-Fallback ohne weitere Caller-Grenze akzeptiert fremde autoritative Pane.

**Warum bleibt oder endet der Zustand?** Falsche Zuordnung bleibt bis unabhängige Identitätsklärung; echte Überlappung verlangt weiteren Start.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Ursprüngliche Taskfakten können bleiben, aktuelle Sessionzuordnung ist nicht verlässlich.

**Zu prüfen:** Obligatorische Caller-Prüfung weist den fehlenden Identitätsbeleg vor Mutation zurück.

**Architekturfolge für diesen Stressor:** Pane-ID nur als Laufzeitadresse behandeln und fehlende UUID nicht als positive Gleichheit. Generation und Identität müssen vor Lifecycle-Mutation verbindlich geprüft werden.

<a id="s033"></a>
## S033 — Transcript-Dateiname verliert die erwartete UUID

**Ursprung:** operations; A6-Zustände: OP025, OP001, OP015. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s033.b01"></a>
### S033.B01 — Unabhängiger Sessionbeleg überlebt Dateiumbenennung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Gültige unabhängig gebundene Beobachtungsgeneration/Session-ID vorhanden und obligatorisch geprüft.

**Warum bleibt oder endet der Zustand?** Namensparser kann scheitern ohne Sessionkontinuität zu verlieren.

**Zugeordnete Residues:** [OPR014: Korrelierte Beobachtung mit Frischegrenze](residues.md#opr014)

**Was bleibt warum nutzbar?** Lifecycle-Verbraucher kann die echte Session weiter identifizieren.

**Zu prüfen:** Zusätzlicher Beleg stammt nur aus dem umbenannten Dateinamen oder wird nicht geprüft.

<a id="s033.b02"></a>
### S033.B02 — Dateiname war einziger Diskriminator

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Parser liefert None, kein unabhängiger Identitätsbeleg, alter Leasebestand bleibt lesbar.

**Warum bleibt oder endet der Zustand?** Bis korrelierter Beleg vorliegt bleibt Zuordnung offen; Vertrauen aufgrund Autorität allein wäre Fehlassoziation.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Verwalteter Startentscheider kann Workspace weiterhin reserviert halten, nicht die Pane als alte Session bestätigen.

**Zu prüfen:** Fehlende UUID wird als positiver Identitätsbeweis akzeptiert oder alter Lease fehlt.

**Architekturfolge für diesen Stressor:** Transcript-Dateiname darf nicht einzige Herkunftsbindung sein. Alternative Identität muss unabhängig authentisch sein, nicht aus dem gleichen kaputten Namen geraten werden.

<a id="s034"></a>
## S034 — Hook fehlt aber Screen-Parser meldet idle

**Ursprung:** operations; A6-Zustände: OP001, OP015, OP024. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s034.b01"></a>
### S034.B01 — Hookloses idle ändert Task nicht

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Aufruf nutzt historisches C11-NoChange und C3-Belegung, keine manuelle Umgehung.

**Warum bleibt oder endet der Zustand?** Alter nichtterminaler Auftrag bleibt belegt bis echte Resultat- oder Autorisierungsevidenz.

**Zugeordnete Residues:** [OPR029: Transaktionale Worker-Belegung](residues.md#opr029), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Assignment-Entscheider behält richtige Belegung und Taskfakten; tatsächlicher Worker kann weiterarbeiten.

**Zu prüfen:** Screen-idle allein gibt den belegten Worker frei oder committet Taskende.

<a id="s034.b02"></a>
### S034.B02 — UI verkauft schwache Anzeige als aktuelle Wahrheit

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Zusätzlicher UI-/Caller-Pfad ignoriert fehlende Hookautorität und behauptet idle verbindlich.

**Warum bleibt oder endet der Zustand?** Irreführende Anzeige bleibt bis korrelierter Refresh; daraus folgt noch keine falsche Außenwirkung.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Kernfakten können richtig weiterbestehen, Beobachterentscheidung aus der Anzeige nicht.

**Zu prüfen:** Anzeige markiert Quelle und Ungewissheit deutlich und erlaubt keinen autoritativen Schluss.

**Architekturfolge für diesen Stressor:** Adapter-NoChange auch an Caller und UI-Frischegrenze erhalten. Screen-idle ist weder Taskabschluss noch Freigabe des Workers.

<a id="s035"></a>
## S035 — Harness meldet done nach einem Zwischenturn

**Ursprung:** operations; A6-Zustände: OP001, OP005. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s035.b01"></a>
### S035.B01 — Zwischenturn beendet Task nicht

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Adapter C11 liefert bei done NoChange, Task bleibt nichtterminal und Arbeit setzt sich fort.

**Warum bleibt oder endet der Zustand?** Nächster Turn oder separate geprüfte Completion bestimmt Ende.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001), [OPR029: Transaktionale Worker-Belegung](residues.md#opr029)

**Was bleibt warum nutzbar?** Taskabsicht und Belegung bleiben erhalten statt durch Turnsignal ersetzt.

**Zu prüfen:** done allein führt zur Completion oder neuen Assignmentfreigabe.

<a id="s035.b02"></a>
### S035.B02 — Zusätzlicher Verbraucher akzeptiert Zwischenstand als Ende

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Unabhängiger Completion-Aufrufer deutet done ohne aufgabenspezifische Endbedingung als final.

**Warum bleibt oder endet der Zustand?** Falsch akzeptierter Zwischenstand bleibt bis sachliche Gegenprüfung.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Gespeicherte Behauptung bleibt nachvollziehbar, fachlicher Abschluss ist nicht erhalten.

**Zu prüfen:** Auftrag war tatsächlich bereits vollständig erfüllt oder verpflichtender Endvalidator verweigert Annahme.

**Architekturfolge für diesen Stressor:** Turnabschluss strikt von Resultatannahme trennen. Verwaiste Bytes sind nur bei echter Dauerhaftigkeit und Zuordnung zu prüfen, nicht bei jedem done zu erfinden.

<a id="s036"></a>
## S036 — Alte autoritative Beobachtung trifft nach neuer Session ein

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s036.b01"></a>
### S036.B01 — Alte Generation bleibt historische Beobachtung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Sessiongeneration und monotone Sequenz werden vor Mutation geprüft, alte Observation ist eindeutig früher.

**Warum bleibt oder endet der Zustand?** Zustellung ist abgearbeitet ohne neue Session zu verändern; historischer Beleg bleibt als alt lesbar.

**Zugeordnete Residues:** [OPR014: Korrelierte Beobachtung mit Frischegrenze](residues.md#opr014)

**Was bleibt warum nutzbar?** Operator kann Vergangenheit prüfen ohne aktuelle Liveness daraus abzuleiten.

**Zu prüfen:** Verspätete Observation ändert neue Session oder wird als frisch angezeigt.

<a id="s036.b02"></a>
### S036.B02 — Gleiche oder fehlende Identität ohne Zeitbindung

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Nur Autoritätsflag und unzureichende UUID, keine verbindliche Generation/Sequenz am Caller bekannt.

**Warum bleibt oder endet der Zustand?** Korrekte Zuordnung kann aus diesen Angaben allein nicht entstehen; konkretes Mutationsverhalten bleibt offen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für frische Sessionbeobachtung kein belegter Gegenstand. Historisches C4 allein beantwortet die End-to-end-Frage nicht.

**Zu prüfen:** Caller-Level-Test mit neuer Session und später alter Observation zeigt verpflichtende Ablehnung oder konkrete Fehlmutation.

<a id="s036.b03"></a>
### S036.B03 — Alte Observation verändert tatsächlich neue Session

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Zusätzliche konkrete Annahme: Caller akzeptiert verspätete autoritative Observation ohne Generation/Sequenzprüfung und schreibt deren Status auf die neue Session; Taskkern bleibt unabhängig intakt.

**Warum bleibt oder endet der Zustand?** Falscher neuer Status bleibt bis korrelierter frischer Gegenbeobachtung oder expliziter Korrektur. Keine automatisch selbsttragende Beobachtungsschleife behauptet.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskauftrag und erhaltene frühere Annahmeangaben bleiben lesbar, aktuelle Sessionattribution ist verletzt. Dies ist ein bedingter Callerfehler, kein aus C4 bewiesener Produktvorfall.

**Zu prüfen:** Isolierte Zustellung derselben alten Observation kann wegen obligatorischer Callerbindung die neue Session nicht verändern.

**Architekturfolge für diesen Stressor:** Verbindliche Observation-Generation und Sequenz am mutierenden Caller festlegen. Quellenautorität ohne Frische darf neue Sessions nicht verändern.

<a id="s037"></a>
## S037 — Zwei Case-Varianten desselben Workspace werden parallel reserviert

**Ursprung:** operations; A6-Zustände: OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s037.b01"></a>
### S037.B01 — Zweite Case-Reservation wird verweigert

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Schreibweisen lösen stabil auf dieselbe device/inode-Identität, beide benutzen C10 und denselben Store.

**Warum bleibt oder endet der Zustand?** Erste Reservation bleibt, zweiter Request endet ohne neue Belegung; erste Arbeit ist damit noch nicht fertig.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Startentscheider besitzt nutzbare exklusive verwaltete Belegung trotz Aliasnamen.

**Zu prüfen:** Zwei gleichzeitige begin_start-Aufrufe reservieren denselben stabilen FileId erfolgreich.

**Architekturfolge für diesen Stressor:** Ein-Store-FileId-Exklusivität als begrenzten Vertrag bewahren. Keine Ausdehnung auf wechselnde Verzeichnisobjekte oder unabhängige Clones.

<a id="s038"></a>
## S038 — Ein untergeordneter Prozess überlebt den gestoppten Harness

**Ursprung:** operations; A6-Zustände: OP016, OP001, OP034. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s038.b01"></a>
### S038.B01 — Nachkomme bleibt unverwalteter Schreiber

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Kindprozess lebt nach Harnessende weiter und kann betroffene Dateien schreiben; Kernstore ist unbetroffen.

**Warum bleibt oder endet der Zustand?** Schreibkonflikt dauert bis Fähigkeit erlischt oder koordiniert übergeben wird; keine automatische Wiederkehr nötig.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskfakten sind noch nutzbar, Workspaceexklusivität fehlt auch wenn Pane weg ist.

**Zu prüfen:** Kind lebt zwar, besitzt aber keine relevante Schreibfähigkeit.

<a id="s038.b02"></a>
### S038.B02 — Unklarer Nachkomme verhindert verwaltete Wiedervergabe

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Entworfene Freigabegrenze verlangt Schreibfähigkeitsklärung und erhält Lease bis dahin.

**Warum bleibt oder endet der Zustand?** Fehlender Prozessbaumbeleg hält Wiederstart an, beendet aber Kind nicht.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Startentscheider kann alte Belegung konservativ nutzen ohne Prozess-Tod zu behaupten.

**Zu prüfen:** Ein anderer Recovery-Pfad gibt Lease ohne diesen Beleg frei.

<a id="s038.b03"></a>
### S038.B03 — Fertige Arbeitsbytes überleben nach bestätigtem Prozessende

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle Schreiber beendet, geflushte Datei mit unabhängiger Runzuordnung vorhanden, Result-Commit fehlt.

**Warum bleibt oder endet der Zustand?** Abnahme wartet auf fachliche Prüfung und berechtigte Completion.

**Zugeordnete Residues:** [OPR006: Zuordenbare verwaiste Arbeitsbytes](residues.md#opr006)

**Was bleibt warum nutzbar?** Prüfer kann vorhandene Arbeit verwerten statt blind neu rechnen.

**Zu prüfen:** Datei ist ungeflusht unzuordenbar oder semantisch unbrauchbar.

**Architekturfolge für diesen Stressor:** Prozessende und Erlöschen sämtlicher Schreibrechte getrennt nachweisen. Keine OS-Fencinggarantie aus einer Sessionrow ableiten.

<a id="s039"></a>
## S039 — Temporärer Worker wartet tagelang auf Permission

**Ursprung:** operations; A6-Zustände: OP017, OP031, OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s039.b01"></a>
### S039.B01 — Permission-Warten hält Worker belegt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Temporärer Worker besitzt nichtterminalen blocked Task, C3-Prüfung bleibt auf jedem betrachteten Assignmentpfad.

**Warum bleibt oder endet der Zustand?** Zeitablauf allein hebt Permission oder Belegung nicht auf.

**Zugeordnete Residues:** [OPR029: Transaktionale Worker-Belegung](residues.md#opr029), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Task und Assignment bleiben inspizierbar; andere unabhängige Worker können getrennt arbeiten.

**Zu prüfen:** Neues Assignment nutzt denselben Worker während blocked fortbesteht.

<a id="s039.b02"></a>
### S039.B02 — Befugter Vertreter kann echte Permissionfrage übernehmen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Zugängliches verständliches Dossier nennt benötigte Entscheidung und keine unauflösbare Owner-Exklusivität.

**Warum bleibt oder endet der Zustand?** Warten endet durch tatsächliche kompetente Entscheidung, nicht durch bloße Kenntnis des blocked-Status.

**Zugeordnete Residues:** [OPR032: Benutzbarer Entscheidungsübergabestand](residues.md#opr032)

**Was bleibt warum nutzbar?** Vertreter hat einen nutzbaren Fallstand; alte Workerbelegung endet dadurch nicht automatisch.

**Zu prüfen:** Vertreter benötigt trotz Dossier fehlendes Ownerwissen oder besitzt keine nötige Autorität.

**Architekturfolge für diesen Stressor:** Temporäre Lebensdauer nicht als automatischen Permission-Timeout auslegen. Belegung und benötigter menschlicher Entscheidungsstand sind getrennte Strukturen.

<a id="s040"></a>
## S040 — Herdr-Restart wird fälschlich als kompletter Maschinenneustart klassifiziert

**Ursprung:** operations; A6-Zustände: OP001, OP016, OP015. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s040.b01"></a>
### S040.B01 — Positive Korrelation verbindet weiterlebende Session

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Gleiche echte Session und frische gebundene Beobachtung sind vorhanden.

**Warum bleibt oder endet der Zustand?** Beobachtungslücke endet ohne Ersatzstart.

**Zugeordnete Residues:** [OPR014: Korrelierte Beobachtung mit Frischegrenze](residues.md#opr014)

**Was bleibt warum nutzbar?** Operator erhält aktuellen korrelierten Sessionbezug statt neuer Identität durch Klassifikationsfehler.

**Zu prüfen:** Korrelation basiert nur auf wiedervergebener Pane oder fehlender UUID.

<a id="s040.b02"></a>
### S040.B02 — Fehlende Evidenz hält Belegung konservativ

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Verwendeter Recovery-Pfad behält Lease statt presumed-gone-Replacement als Freigabe zu verwenden.

**Warum bleibt oder endet der Zustand?** Bis alter Schreiber geklärt ist kein zweiter verwalteter Start.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Belegung bleibt nutzbar, behauptet aber keine aktuell sichtbare Runtime.

**Zu prüfen:** Alternative Reconnectfunktion oder Caller gibt Lease dennoch frei.

<a id="s040.b03"></a>
### S040.B03 — Presumption und späterer Start schaffen Überlappung

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** C4-artige Presumption erzeugt Ersatzrecord, Caller startet später Ersatz und alter Harness schreibt noch.

**Warum bleibt oder endet der Zustand?** Überlappung dauert mit beiden Schreibfähigkeiten, nicht allein mit falschem Restartlabel.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Unbetroffener Taskkern bleibt, exklusive Arbeitsattribution geht verloren.

**Zu prüfen:** Kein Ersatzlaunch erfolgt oder alte Schreibfähigkeit war vor ihm erloschen.

**Architekturfolge für diesen Stressor:** Herdr-Restart nicht mit Maschinenneustart gleichsetzen. C4-Replacement ist nur ein Datensatz; Konfliktschaden braucht nachfolgenden Launch und lebenden alten Schreiber.

<a id="s041"></a>
## S041 — Plugin schreibt Debugtext auf stdout zwischen Protokollframes

**Ursprung:** operations; A6-Zustände: OP018, OP022. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s041.b01"></a>
### S041.B01 — Framingfehler endet in begrenzter Aktivierungssperre

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Host erkennt Debugtext als Protokollverletzung, persistentes Restartbudget greift, Fehlerjournal bleibt außerhalb Plugin lesbar.

**Warum bleibt oder endet der Zustand?** Nach endlichen Fehlversuchen kein weiterer automatischer Start bis ausdrücklicher neuer Entscheidung.

**Zugeordnete Residues:** [OPR017: Begrenztes Aktivierungs-Fehlerjournal](residues.md#opr017)

**Was bleibt warum nutzbar?** Operator kann Defekt und verbrauchtes Budget diagnostizieren, nicht Pluginheilung aus Restart folgern.

**Zu prüfen:** Debugtext erzeugt akzeptierte Fake-Frames oder Budget setzt sich bei jedem Start zurück.

<a id="s041.b02"></a>
### S041.B02 — Protokollbruch lässt Außenwirkung offen

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Intent lag vor möglicher Teilnehmerannahme dauerhaft vor, gültige Antwort geht im kaputten Stream verloren.

**Warum bleibt oder endet der Zustand?** Sperren des Plugins klärt Annahme nicht; benötigt unabhängige verbindliche Quittung.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator behält genaue Operation zum späteren Abgleich ohne blinden Retry.

**Zu prüfen:** Host markiert wegen Framingfehler sicher nicht angenommen oder verliert Operation-ID.

**Architekturfolge für diesen Stressor:** Stdout strikt als Framingkanal behandeln, Diagnose auf stderr. Aktivierungsfehler und möglicher Teilnehmererfolg sind getrennte Journale mit getrennten Ausgängen.

<a id="s042"></a>
## S042 — Plugin meldet einen Frame von mehreren Terabytes an

**Ursprung:** operations; A6-Zustände: OP018, OP021, OP008, OP002. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s042.b01"></a>
### S042.B01 — Riesenheader wird vor Allokation begrenzt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Größenlimit und reservierte Kernressourcen greifen vor Lesen/Allokation des Terabyte-Bodys.

**Warum bleibt oder endet der Zustand?** Betroffene Aktivierung bleibt gesperrt, unabhängige lokale Arbeit hat weiter Ressourcen.

**Zugeordnete Residues:** [OPR018: Reservierte unabhängige Hostkapazität](residues.md#opr018), [OPR017: Begrenztes Aktivierungs-Fehlerjournal](residues.md#opr017)

**Was bleibt warum nutzbar?** Kernel kann weiter bedienen und Operator den Headerverstoß prüfen.

**Zu prüfen:** Residenter Speicher wächst mit deklarierter Terabytelänge oder lokale Aufnahme fällt mit aus.

<a id="s042.b02"></a>
### S042.B02 — Unbegrenzte Allokation erschöpft Host

**Art:** eskalation. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Host reserviert oder liest unkontrolliert nach Längenangabe, keine wirksamen vorgelagerten Grenzen.

**Warum bleibt oder endet der Zustand?** Speicherverbrauch steigt bis OOM/Stall, nicht als begrenzter Attraktor behauptet.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Während globaler Erschöpfung ist keine unabhängige Bedienfähigkeit belegt; spätere Datenerhaltung hängt von konkret überlebendem Store ab.

**Zu prüfen:** Überlanger Header wird nachweislich mit beschränktem Speicher verarbeitet und lokale Arbeit geht weiter.

**Architekturfolge für diesen Stressor:** Deklarierte Länge inklusive Integergrenzen vor Allokation prüfen. Framebudget muss mit Gesamtressourcen zusammenpassen; Pluginprozess allein ist kein Speicherschutz.

<a id="s043"></a>
## S043 — Plugin beantwortet Healthchecks aber blockiert jede Geschäftsoperation

**Ursprung:** operations; A6-Zustände: OP020, OP022, OP004, OP021. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s043.b01"></a>
### S043.B01 — Gesunder Healthpfad, blockierte Geschäftsarbeit

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Health antwortet, Geschäftsdefekt blockiert weiterhin, begrenzte Originalqueue und lokale Ressourcen bleiben intakt.

**Warum bleibt oder endet der Zustand?** Defekt hält Geschäftsservice an; Health allein beendet ihn nicht.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010), [OPR009: Unabhängiger lokaler Bedienpfad](residues.md#opr009)

**Was bleibt warum nutzbar?** Lokaler Operator kann wartende Originalaufträge inspizieren und unabhängige lokale Arbeit bedienen, nicht blockierte Operationen abschließen.

**Zu prüfen:** Auch lokale unabhängige Arbeit hängt am Geschäftsdeadlock oder Queue geht verloren.

<a id="s043.b02"></a>
### S043.B02 — Geschäftstimeout mit möglicher Annahme

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Aufruf erreichte vielleicht Teilnehmer, Intent ist dauerhaft, kein verbindlicher Ausgang.

**Warum bleibt oder endet der Zustand?** Fristablauf beendet Warten auf Pipe nicht das unbekannte Geschäftsergebnis.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Operation bleibt gezielt rekonzilierbar, Healthsuccess liefert keinen Ergebnisbeleg.

**Zu prüfen:** Verbindliche korrelierte Quittung war vorhanden oder Timeout löst neue unkoordinierte Wirkung aus.

<a id="s043.b03"></a>
### S043.B03 — Timeoutlast verstärkt Businessblockade

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Timeouts erzeugen zusätzliche Calls, diese verlängern Warteschlangen und verursachen neue Timeouts; begrenztes Hochlastregime bleibt nach initialem Defekt bei gleichem Grundbedarf.

**Warum bleibt oder endet der Zustand?** Stau → Timeout → Calllast → weniger Nutzfortschritt → Stau. Bei permanentem festem Deadlock nur äußerer Defekt, ohne Rückwirkung keine Hypothese.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Lesbarer Taskkern bleibt nur bei nicht erschöpftem Store; gesunde Geschäftsarbeit ist nicht behauptet.

**Zu prüfen:** Bei festem Grundbedarf nach Defektentfernung drainen hohe und niedrige Anfangslast gleich oder Retrylast beeinflusst Fortschritt nicht.

**Architekturfolge für diesen Stressor:** Businessfortschritt als operationsbezogene Evidenz erfassen statt aus Health ableiten. Bounded timeouts sind keine Nichtannahmebelege.

<a id="s044"></a>
## S044 — Plugin crasht sofort nach jedem automatischen Restart

**Ursprung:** operations; A6-Zustände: OP019, OP018. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s044.b01"></a>
### S044.B01 — Endliche Restartfolge endet diagnostizierbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Gleicher Defekt und endliches nicht bei Hostrestart zurückgesetztes Budget, Journal zugänglich.

**Warum bleibt oder endet der Zustand?** Crash → Restart gilt nur bis Budgetende, danach kein automatischer Neustart.

**Zugeordnete Residues:** [OPR017: Begrenztes Aktivierungs-Fehlerjournal](residues.md#opr017)

**Was bleibt warum nutzbar?** Operator behält konkreten Fehler und verbrauchte Versuche zur gezielten Änderung.

**Zu prüfen:** Hostrestart setzt Budget zurück oder unverändertes Plugin startet unbegrenzt automatisch.

<a id="s044.b02"></a>
### S044.B02 — Unbegrenzte Policy wiederholt denselben Crash

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Policy hat kein dauerhaftes Budget oder fortgesetzte externe Resets erneuern es; Kernressourcen sind anderweitig geschützt.

**Warum bleibt oder endet der Zustand?** Crash triggert Policyrestart und denselben Crash; technisch erzwungener Zyklus ohne belegte Attraktion.

**Zugeordnete Residues:** [OPR009: Unabhängiger lokaler Bedienpfad](residues.md#opr009)

**Was bleibt warum nutzbar?** Lokaler unabhängiger Bedienpfad bleibt nur wegen separater Ressourcen und Aktivierungsunabhängigkeit nutzbar.

**Zu prüfen:** Restartchurn erschöpft gemeinsamen Host oder Plugin liefert stabil nützliche Arbeit.

**Architekturfolge für diesen Stressor:** Restartbudget an Aktivierungsidentität dauerhaft binden. Ein Operatorreset ist externer Eingang, kein Beweis selbstheilender Pluginaufsicht.

<a id="s045"></a>
## S045 — Optionales Pluginmanifest ist syntaktisch kaputt

**Ursprung:** operations; A6-Zustände: OP018, OP026, OP021, OP002. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s045.b01"></a>
### S045.B01 — Optionaler Parsefehler lässt lokalen Betrieb intakt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Bundled lokaler Pfad und Kernel starten ohne fehlerhaftes optionales Manifest, dessen Fehler wird separat erfasst.

**Warum bleibt oder endet der Zustand?** Betroffene Aktivierung gesperrt bis korrigierte Eingabe und bewusster Retry, lokale Arbeit läuft unabhängig.

**Zugeordnete Residues:** [OPR017: Begrenztes Aktivierungs-Fehlerjournal](residues.md#opr017), [OPR009: Unabhängiger lokaler Bedienpfad](residues.md#opr009)

**Was bleibt warum nutzbar?** Operator kann lokal den konkreten Aktivierungsfehler und Tasks prüfen.

**Zu prüfen:** Safe-Mode lädt zwingend das fehlerhafte Manifest und beendet global den Start.

<a id="s045.b02"></a>
### S045.B02 — Globaler Parser verhindert jeden Start

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Auch lokale Recovery aktiviert zwingend denselben defekten Manifestparser, Datenbestand bleibt physisch lesbar.

**Warum bleibt oder endet der Zustand?** Wiederholter Start scheitert bis Konfiguration oder Parsepfad geändert wird.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskfakten können nach zulässigem Wiederzugang weiter nützlich sein, aktueller lokaler API-Zugang ist nicht erhalten.

**Zu prüfen:** Bundled-only-Start umgeht den Defekt bereits.

**Architekturfolge für diesen Stressor:** Optionales Manifest erst in isolierter Aktivierung auswerten und lokalen Safe-Mode nicht von dessen erfolgreichem Parse abhängig machen.

<a id="s046"></a>
## S046 — Plugin liefert Erfolg vor dauerhaftem Commit beim Teilnehmer

**Ursprung:** operations; A6-Zustände: OP005, OP022, OP001, OP028. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s046.b01"></a>
### S046.B01 — Frühe Erfolgsmeldung wird als Behauptung gehalten

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Plugin meldet vor dauerhaftem Teilnehmercommit Erfolg, Intent und empfangene Behauptung sind erhalten, keine verbindliche Quittung.

**Warum bleibt oder endet der Zustand?** Bis echter dauerhafter Ausgang belegt ist bleibt Abschluss offen.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator kennt genaue Operation und unzureichenden Beleg statt erfolgreich zu halluzinieren.

**Zu prüfen:** Vorzeitige Meldung wird als dauerhafte Annahme ausgegeben oder Operation-ID fehlt.

<a id="s046.b02"></a>
### S046.B02 — Teilnehmer committet später verbindlich

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Nach früher Meldung erfolgt tatsächlich dauerhafter Commit mit zugänglicher passender Quittung.

**Warum bleibt oder endet der Zustand?** Für diese Wirkung ist Ausgang geklärt, nicht rückwirkend Verlässlichkeit früherer Meldungen bewiesen.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann konkreten Endzustand belegen ohne nochmals auszulösen.

**Zu prüfen:** Teilnehmerrestart verliert die angeblich dauerhafte Operation.

<a id="s046.b03"></a>
### S046.B03 — Falscher lokaler Erfolg überlebt verlorene Teilnehmerarbeit

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Factory akzeptiert vorzeitiges Erfolgssignal, Teilnehmer verliert Arbeit vor Commit, lokale Taskdaten bleiben.

**Warum bleibt oder endet der Zustand?** Falsche Completion bleibt bis Gegenbeleg und explizite Korrektur.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Annahmebehauptung ist nachvollziehbar, ausgeführte Geschäftsarbeit ist gerade nicht erhalten.

**Zu prüfen:** Teilnehmer belegt doch einen dauerhaften Commit vor dem Verlust.

**Architekturfolge für diesen Stressor:** Teilnehmer-Erfolg nur nach konkret zugesicherter Commitsemantik werten. Lokales Journal bewahrt eine Behauptung, kann ihre entfernte Wahrheit nicht herstellen.

<a id="s047"></a>
## S047 — Plugin-Unterprozess verbraucht alle File Descriptors

**Ursprung:** operations; A6-Zustände: OP018, OP021, OP008, OP002. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s047.b01"></a>
### S047.B01 — Plugin erreicht eigenes FD-Budget

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Limit und Kernreserve wirksam, Fehlerjournal benötigt keine erschöpften Plugin-FDs.

**Warum bleibt oder endet der Zustand?** Plugin scheitert begrenzt, Kern hat weiterhin Dateizugriff und lokales Socket.

**Zugeordnete Residues:** [OPR018: Reservierte unabhängige Hostkapazität](residues.md#opr018), [OPR017: Begrenztes Aktivierungs-Fehlerjournal](residues.md#opr017)

**Was bleibt warum nutzbar?** Andere Aktivierungen und Operator behalten konkrete Ressourcen sowie Diagnose.

**Zu prüfen:** Plugin erschöpft trotzdem gemeinsame FDs und verhindert lokale Taskabfrage.

<a id="s047.b02"></a>
### S047.B02 — Geteiltes FD-Limit blockiert Kern

**Art:** extern-erzwungen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Keine genügende globale Reserve, Plugin hält FDs die Store und Socketzugriff verhindern.

**Warum bleibt oder endet der Zustand?** Ressourcen bleiben bis tatsächlicher Freigabe oder Prozessende blockiert; ein Statuswechsel allein schließt keine FDs.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Aktuelle lokale Bedienfähigkeit ist nicht belegt. Bereits gespeicherte Daten könnten nach Freigabe noch nutzbar sein, sind hier aber nicht zugänglich.

**Zu prüfen:** Isolierter FD-Sturm lässt Kernzugriffe messbar weiterlaufen.

**Architekturfolge für diesen Stressor:** FD-Aufnahme pro Prozess und gemeinsame OS-Reserve prüfen. Getrennte Prozesse teilen Systemlimits und begründen allein keinen unabhängigen lokalen Betrieb.

<a id="s048"></a>
## S048 — Kompatibles Major-Protokoll ändert die Bedeutung eines optionalen Felds

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s048.b01"></a>
### S048.B01 — Optionales Feld ist für alte Operation sicher ignorierbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkreter Vertrag zeigt dass Ignorieren keine Bedeutung der betreffenden Operation ändert, beide Peers halten ihn ein.

**Warum bleibt oder endet der Zustand?** Versionswechsel führt nicht zu Bedeutungsdrift auf diesem Pfad.

**Zugeordnete Residues:** [OPR019: Festgehaltener semantischer Operationsvertrag](residues.md#opr019)

**Was bleibt warum nutzbar?** Host kann genau die weiterhin gleichbedeutende Operation nutzen.

**Zu prüfen:** Ein altes/neues Nachrichtenpaar mit gleicher Absicht ergibt verschiedene relevante Wirkungen.

<a id="s048.b02"></a>
### S048.B02 — Neue Feldbedeutung wird vor Geschäftsaufruf abgelehnt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Beispielsweise änderte optionale Preis-/Dry-run-Bedeutung eine Wirkung, erhaltener semantischer Vertrag erkennt Inkompatibilität verpflichtend.

**Warum bleibt oder endet der Zustand?** Aktivierung dieser Operation wartet auf passenden Peer oder bewusste Vertragsänderung.

**Zugeordnete Residues:** [OPR019: Festgehaltener semantischer Operationsvertrag](residues.md#opr019)

**Was bleibt warum nutzbar?** Bearbeiter kann den konkreten Bedeutungsunterschied prüfen statt Major-Kompatibilität als Freigabe nutzen.

**Zu prüfen:** Handshake akzeptiert und Geschäftsaufruf nutzt widersprüchliche Bedeutung trotz Vertragsbindung.

<a id="s048.b03"></a>
### S048.B03 — Nur gleiche Majorzahl, konkrete Bedeutung unbekannt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Weder betroffenes Feld noch bisherige Bedeutung oder beeinträchtigte Invariante sind genannt.

**Warum bleibt oder endet der Zustand?** Ohne Nachrichtenpaar und Invariante keine Aussage über Schaden oder nützlichen kompatiblen Restpfad.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für semantisch sichere Operation ist kein nachgewiesener Vertrag vorhanden; keine beliebige Gefahr als Tatsache.

**Zu prüfen:** Ein spezifiziertes altes/neues Paar entscheidet sichere Ignorierbarkeit oder eine konkrete Verletzung.

<a id="s048.b04"></a>
### S048.B04 — Geänderte optionale Bedeutung lässt echte statt simulierte Wirkung zu

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur als konkretes Gegenbeispiel: Ein optionales Feld bezeichnet beim neuen Peer wirkungsverhindernde Simulation, beim alten nur ignorierbare Diagnose. Trotz gleicher Majorzahl wird derselbe Geschäftsaufruf real angenommen; lokaler genauer Intentbezug überlebt.

**Warum bleibt oder endet der Zustand?** Die nicht beabsichtigte reale Annahme bleibt als historische Wirkung, auch nachdem Peers Bedeutungen angleichen. Ein einzelner semantischer Vertragsbruch ist kein Attraktor.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator kann die beabsichtigte simulierte Operation und erhaltene Versuche prüfen, nicht aus dem Majorhandshake sichere gleiche Bedeutung ableiten. Dieses Feld ist eine stipulierte Testannahme, kein behauptetes bestehendes Factoryfeld.

**Zu prüfen:** Alter und neuer Peer interpretieren das konkret festgelegte Feld gleich oder verpflichtende Vertragsprüfung verhindert jede reale Annahme der als simuliert gemeinten Operation.

**Architekturfolge für diesen Stressor:** Konkrete Semantik optionaler Felder neben Major und Schema vertraglich prüfen. Beispielhafte Preis-/Dry-run-Alternativen sind neue bedingte Entwürfe, keine Aussage über ein tatsächlich vorhandenes Feld.

<a id="s049"></a>
## S049 — Ein Scope deaktiviert Plugin während dessen Effekt in flight ist

**Ursprung:** operations; A6-Zustände: OP028, OP022, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s049.b01"></a>
### S049.B01 — Deaktivierte Capability hat offenen Außenwirkungsstand

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Neue Dispatches gestoppt, alte Operation könnte angenommen sein, Intentjournal bleibt ohne erreichbaren Ergebnisweg.

**Warum bleibt oder endet der Zustand?** Ungewissheit dauert bis zulässiger Ergebniszugang oder bewusste Disposition, Disable allein ist kein Ausgang.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Operator kann offene konkrete Verpflichtung sehen obwohl Plugin nicht mehr neu arbeiten darf.

**Zu prüfen:** Disable löscht Intent oder meldet alle in-flight Wirkungen automatisch nicht geschehen.

<a id="s049.b02"></a>
### S049.B02 — Altwirkung bleibt verbindlich belegt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Teilnehmerquittung für bereits angenommene irreversible Wirkung ist unabhängig von neuer Pluginaktivierung zugänglich.

**Warum bleibt oder endet der Zustand?** Historische Annahme bleibt nach Disable wahr, weitere Wirkungen bleiben getrennt gesperrt.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann genau diesen Abschluss nachweisen, keine universelle Teilnehmerstilllegung.

**Zu prüfen:** Quittung ist nur lokales vorläufiges Erfolgssignal oder nicht mehr erreichbar.

**Architekturfolge für diesen Stressor:** Deaktivierung als Ende neuer Dispatchrechte modellieren, nicht als Rücknahme bereits angenommener Wirkungen. Rekonziliationsrecht und Aufbewahrungshorizont separat erhalten.

<a id="s050"></a>
## S050 — Plugin behauptet reversibel aber inverse Operation verliert Daten

**Ursprung:** operations; A6-Zustände: OP030, OP006, OP026, OP017. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s050.b01"></a>
### S050.B01 — Verlustbehaftete Inverse wird nicht ausgeführt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Erhaltener Kompensationsstand und unabhängiger operationstypischer Vergleich zeigen dass Inverse benötigte Information verlieren würde; Grenze verweigert sie.

**Warum bleibt oder endet der Zustand?** Wartet auf sichere konkrete Alternative oder bewusste neue Geschäftsentscheidung.

**Zugeordnete Residues:** [OPR023: Operationsspezifischer Kompensationsstand](residues.md#opr023), [OPR034: Unabhängiger aufgabenbezogener Prüfbezug](residues.md#opr034)

**Was bleibt warum nutzbar?** Bearbeiter kann den bestimmten Vorzustand und Defekt der Inverse prüfen.

**Zu prüfen:** Inverse wird trotz fehlender Erhaltung akzeptiert oder Prüfbezug ist selbst dieselbe Behauptung des Plugins.

<a id="s050.b02"></a>
### S050.B02 — Inverse löscht einzigartigen Inhalt

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Deklaration wird ungeprüft geglaubt, inverse Wirkung zerstört einzige benötigte neuere oder ursprüngliche Information ohne unabhängige Kopie.

**Warum bleibt oder endet der Zustand?** Beendete Inverse hinterlässt bleibende Informationslücke.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für den zerstörten einzigartigen Inhalt keines; keine imaginäre Vorzustandskopie im Journal.

**Zu prüfen:** Eine tatsächlich erhaltene vollständige Quelle rekonstruiert den Inhalt.

<a id="s050.b03"></a>
### S050.B03 — Schädliche Inverse mit erhaltenem Vorzustandsbeleg

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Inverse ist schon ausgeführt, operationsspezifischer vollständiger Vorzustand und Versuchsstand blieben unabhängig erhalten.

**Warum bleibt oder endet der Zustand?** Schaden besteht bis zulässige konkret geprüfte Wiederherstellung; Beleg allein führt sie nicht aus.

**Zugeordnete Residues:** [OPR023: Operationsspezifischer Kompensationsstand](residues.md#opr023)

**Was bleibt warum nutzbar?** Zuständiger Bearbeiter kann Wiederherstellung beurteilen, aktueller korrekter Teilnehmerzustand ist nicht erhalten.

**Zu prüfen:** Vorzustand enthält gerade die verlorenen Werte nicht oder inverse Bedeutung bleibt unbekannt.

**Architekturfolge für diesen Stressor:** Reversibilität mit konkreten Vor-/Nachbedingungen und Versionsbindung prüfen. Deklaration ersetzt keine verlustfreie Inverse und Journal ist keine Rücknahme.

<a id="s051"></a>
## S051 — Netz partitioniert nach externer Annahme vor Response

**Ursprung:** operations; A6-Zustände: OP022, OP028, OP001, OP023. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s051.b01"></a>
### S051.B01 — Teilnehmer hat angenommen, Factory weiß Ausgang nicht

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Annahme ist im Szenario erfolgt, Response verloren, lokales Intentjournal erhalten und keine zugängliche Quittung.

**Warum bleibt oder endet der Zustand?** Partition und fehlende Evidenz halten lokales Nichtwissen, nicht zwingend entfernte Arbeit an.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator kann genaue Operation benennen und weitere Schritte begrenzen.

**Zu prüfen:** Timeout wird als Nichtannahme gespeichert oder bindender Beleg ist bereits zugänglich.

<a id="s051.b02"></a>
### S051.B02 — Erhaltene Quittung beendet Ergebnisungewissheit

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Nach erreichbarem Lookup liegt verbindliche Quittung für genau die erste Annahme vor.

**Warum bleibt oder endet der Zustand?** Ausgang ist geklärt ohne neue Wirkung; irreversible Annahme kann trotzdem nicht ungeschehen gemacht werden.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann erfolgte Arbeit nutzen und belegen.

**Zu prüfen:** Lookup verwechselt Operation oder Quittung überlebt Teilnehmerrestart nicht.

<a id="s051.b03"></a>
### S051.B03 — Blinder Retry erzeugt zweite Annahme

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Factory wiederholt unbekannte Wirkung, Teilnehmer ignoriert oder erhält keine gemeinsame Idempotenzidentität und nimmt erneut an.

**Warum bleibt oder endet der Zustand?** Doppelte Historie bleibt auch nach Netzwerkheilung, finanzielle Korrektur braucht separate Handlung.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Erhaltene lokale Versuche helfen untersuchen, beweisen aber weder Gesamtzahl noch aktuelle Bilanz.

**Zu prüfen:** Teilnehmer belegt nur eine Annahme trotz Retry.

**Architekturfolge für diesen Stressor:** Netztimeout als unbekannten Ausgang führen und durch verbindliche Operation-ID rekonzilieren. Ein lokaler Retrykey erzwingt keine entfernte Einmaligkeit.

<a id="s052"></a>
## S052 — Provider DNS zeigt auf falschen Endpunkt

**Ursprung:** operations; A6-Zustände: OP020, OP021, OP007, OP027. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s052.b01"></a>
### S052.B01 — Falscher DNS-Endpunkt wird vor Datenübergabe abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Strikte Peer-/Namensprüfung funktioniert, falscher Endpunkt besitzt keine passende Autorität, Kern und lokaler Zugang bleiben unabhängig.

**Warum bleibt oder endet der Zustand?** Solange eine richtige authentische Route fehlt, bleibt der betroffene Providerpfad gesperrt.

**Zugeordnete Residues:** [OPR009: Unabhängiger lokaler Bedienpfad](residues.md#opr009), [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Lokaler Operator kann vorhandene Intentdaten bearbeiten ohne sie dem falschen Ziel zu übergeben.

**Zu prüfen:** Geschützte Bodydaten gelangen trotz fehlgeschlagener Peerprüfung an falschen Endpunkt.

<a id="s052.b02"></a>
### S052.B02 — Falscher Endpunkt erhält tatsächlich Daten oder Wirkung

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Verifikation wird umgangen oder falscher Endpunkt ist ebenfalls akzeptiert, geschützte Daten werden übertragen beziehungsweise falsche Wirkung angenommen.

**Warum bleibt oder endet der Zustand?** Bereits erfolgte Offenlegung oder falsche Zielwirkung bleibt historische Eigenschaft, spätere DNS-Korrektur nimmt sie nicht zurück.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Unbetroffenes Intentjournal kann den beabsichtigten Zielbezug zeigen, nicht verlorene Vertraulichkeit wiederherstellen.

**Zu prüfen:** Keine geschützten Daten oder Wirkungen erreichten das falsche Ziel.

**Architekturfolge für diesen Stressor:** Provideridentität unabhängig von DNS prüfen und konkreten Zielbezug vor Bodyübertragung sichern. Lokale Aufruferauthentifizierung ersetzt keine Provider-TLS-Grenze.

<a id="s053"></a>
## S053 — TLS-Zertifikat läuft ab während langer Offlinephase

**Ursprung:** operations; A6-Zustände: OP020, OP021, OP007, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s053.b01"></a>
### S053.B01 — Abgelaufenes Zertifikat sperrt Providerpfad

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Strikte Ablauf-/Peerprüfung und verlässliche Zeit, lokaler Kernel und Bedienpfad unabhängig gesund.

**Warum bleibt oder endet der Zustand?** Bis authentisches gültiges Zertifikat oder legitim korrigierte Zeit vorliegt keine neue Übertragung.

**Zugeordnete Residues:** [OPR009: Unabhängiger lokaler Bedienpfad](residues.md#opr009)

**Was bleibt warum nutzbar?** Operator kann lokal weiter inspizieren und unabhängige Aufgaben bedienen.

**Zu prüfen:** Lokaler Pfad hängt am selben Zertifikat oder Client ignoriert Ablauf.

<a id="s053.b02"></a>
### S053.B02 — Unsicherer Bypass ermöglicht fremden Peer

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Zusätzlich zum Ablauf wird Prüfung abgeschaltet und fremder Peer empfängt tatsächlich geschützte Inhalte.

**Warum bleibt oder endet der Zustand?** Erfolgte Offenlegung bleibt, ohne tatsächliche Übertragung wäre nur Risiko gegeben.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Unbetroffene Taskfakten können erhalten sein, betroffene Vertraulichkeit nicht.

**Zu prüfen:** Trotz Bypass war Peer unabhängig authentisch oder kein vertraulicher Inhalt wurde übertragen.

**Architekturfolge für diesen Stressor:** Zertifikatsablauf unter vertrauenswürdiger Zeit als spezifischen Transporthalt behandeln. Manuelle Authentifizierungsumgehung ist eine neue riskante Handlung, keine Recoverygarantie.

<a id="s054"></a>
## S054 — WebSocket-Client liest nie seine Events

**Ursprung:** operations; A6-Zustände: OP021, OP024, OP003, OP008, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s054.b01"></a>
### S054.B01 — Nichtlesender Client wird begrenzt abgehängt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle Puffer begrenzt, Kernreserve vorhanden, Clientcursor und Frischegrenze werden erhalten.

**Warum bleibt oder endet der Zustand?** Langsamer Client bleibt getrennt, Kernarbeit geht weiter; spätere Aufholung nur innerhalb tatsächlicher Retention.

**Zugeordnete Residues:** [OPR018: Reservierte unabhängige Hostkapazität](residues.md#opr018), [OBR029: Begrenztes Cursor-Abonnement](residues.md#obr029)

**Was bleibt warum nutzbar?** Die Kernreserve und der begrenzte autorisierte Subscriber-Cursor bleiben nutzbar. Der Cursor bezeichnet vorhandenen Ereignisbereich und mögliche Retentionslücke, nicht eine Runtime-Sessiongeneration.

**Zu prüfen:** Speicher wächst ohne Grenze oder Anzeige verkauft alten Cursor als aktuelle Wahrheit.

**Nachtrag des Koordinators:** [A7S03](../review-dispositions.md#a7s03); ursprüngliche Abgabe unverändert.

<a id="s054.b02"></a>
### S054.B02 — Aufholen aus erhaltenem Eventbestand

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Client liest wieder, letzter Cursor und vollständig benötigter kanonischer Nachlauf sind verfügbar.

**Warum bleibt oder endet der Zustand?** Endlicher Rückstand kann gelesen werden ohne Events als externe Wirkungen erneut auszuführen.

**Zugeordnete Residues:** [OBR029: Begrenztes Cursor-Abonnement](residues.md#obr029), [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Der Subscriber liest den tatsächlich erhaltenen Ereignissuffix zu seinem autorisierten Cursor. Kanonische Eingaben erlauben die Rekonstruktion innerhalb dieses Horizonts, ohne eine Runtimeobservation zu benötigen.

**Zu prüfen:** Retentionslücke fehlt im Replay oder Plugin wird beim Aufholen aufgerufen.

**Nachtrag des Koordinators:** [A7S03](../review-dispositions.md#a7s03); ursprüngliche Abgabe unverändert.

<a id="s054.b03"></a>
### S054.B03 — Unbegrenzte Clientqueue erschöpft gemeinsame Ressourcen

**Art:** eskalation. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Immer neue Events werden ungegrenzt für nichtlesenden Client gepuffert.

**Warum bleibt oder endet der Zustand?** Puffer wächst bis Ressourcenlimit oder Ausfall, keine begrenzte Attraktion belegt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Unabhängige lokale Bedienbarkeit ist ohne Schutz nicht nachgewiesen; vorhandene Datenträgerbits allein ersetzen sie nicht.

**Zu prüfen:** Speicher bleibt trotz Eventzufuhr begrenzt und unabhängige Requests schreiten fort.

**Architekturfolge für diesen Stressor:** Subscriptionbudgets vom Daemonpuffer bis Socket durchsetzen. Ein letzter Cursor erlaubt nur Replay noch vorhandener Events, nicht pauschale Erhaltung aller offenen Vorfälle.

<a id="s055"></a>
## S055 — HTTP-Transport fällt aus während lokaler Betrieb intakt ist

**Ursprung:** operations; A6-Zustände: OP021, OP001, OP031. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s055.b01"></a>
### S055.B01 — Lokale Bedienung bleibt trotz HTTP-Ausfall

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Wie Szenario vorgibt sind Kernel und lokaler Zugriff intakt, berechtigter Mensch ist lokal erreichbar.

**Warum bleibt oder endet der Zustand?** HTTP-Ausfall hält nur diesen Kanal an, unabhängige lokale Aufträge können weitergehen.

**Zugeordnete Residues:** [OPR009: Unabhängiger lokaler Bedienpfad](residues.md#opr009)

**Was bleibt warum nutzbar?** Operator kann konkrete Taskabfragen und erlaubte lokale Arbeit ohne HTTP erledigen.

**Zu prüfen:** HTTP-only-Fehler verhindert auch die zugesagte lokale Abfrage.

<a id="s055.b02"></a>
### S055.B02 — Benötigter Entscheider erreicht nur HTTP

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Lokaler Kernel gesund, zuständiger Mensch nicht vor Ort und kein befugter lokaler Ersatz.

**Warum bleibt oder endet der Zustand?** Human-gated Aufgabe wartet auf erreichbare kompetente Entscheidung, nicht alle Aufgaben stehen.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Lokaler Taskkern bewahrt Entscheidungsbedarf für später, kein erreichbarer Entscheidungskanal wird behauptet.

**Zu prüfen:** Befugter erreichbarer Vertreter löst dieselbe Aufgabe ohne HTTP.

**Architekturfolge für diesen Stressor:** HTTP-Capability getrennt vom transportneutralen Kernel und lokalen Zugang halten. Bei Remote-Only-Operator kann trotzdem menschliche Entscheidung fehlen.

<a id="s056"></a>
## S056 — Remote-Client zeigt tagelang gecachten Status running

**Ursprung:** operations; A6-Zustände: OP024, OP001, OP017, OP034, OP023. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s056.b01"></a>
### S056.B01 — Veralteter Cache bleibt ehrlich als alt nutzbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Letzter Cursor Quelle und Frischegrenze erhalten, UI markiert Nichtwissen über aktuellen Zustand.

**Warum bleibt oder endet der Zustand?** Bis autoritativer Refresh keine aktuelle Livenessaussage; reale Arbeit kann laufen fertig oder blocked sein.

**Zugeordnete Residues:** [OPR014: Korrelierte Beobachtung mit Frischegrenze](residues.md#opr014)

**Was bleibt warum nutzbar?** Operator kann den damaligen beobachteten Stand einordnen ohne ihn als heutige Wahrheit zu nutzen.

**Zu prüfen:** UI zeigt tagelang running ohne erkennbare zeitliche Begrenzung.

<a id="s056.b02"></a>
### S056.B02 — Alter Cache führt zu falschem Glauben

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Client behauptet alte running-Information als aktuell, Kernfakten bleiben unabhängig intakt.

**Warum bleibt oder endet der Zustand?** Glaube bleibt bis korrelierter Refresh oder Gegenbeleg, nicht zwingend Fehler im Workprodukt.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Realer Taskkern ist weiter inspizierbar über einen gültigen Zugang, Cacheentscheidung ist unzuverlässig.

**Zu prüfen:** Aktueller Kernstand entspricht tatsächlich während des ganzen Intervalls der behaupteten Beobachtung.

<a id="s056.b03"></a>
### S056.B03 — Cachebedingter Ersatz dupliziert Geschäft

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Zusätzliche menschliche Aktion erzeugt neuen Effektauftrag ohne alte Operation zu klären, beide werden angenommen.

**Warum bleibt oder endet der Zustand?** Doppelte Wirkung bleibt nach Cachekorrektur; der Cache allein hätte sie nicht erzeugt.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Erhaltene Intentjournale können Absichten nachzeichnen, nicht Einmaligkeit garantieren.

**Zu prüfen:** Kein Ersatzdispatch oder nur eine tatsächliche Teilnehmerannahme.

**Architekturfolge für diesen Stressor:** Cached running als datierte Beobachtung ausgeben, nicht als aktuellen Taskzustand. Ersatzarbeit braucht frische Kern- und Wirkungsprüfung statt UI-Status.

<a id="s057"></a>
## S057 — Netzwerk-Antworten kommen doppelt und vertauscht

**Ursprung:** operations; A6-Zustände: OP026, OP001, OP024, OP025, OP023. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s057.b01"></a>
### S057.B01 — Duplikate und alte Antworten werden korrekt zugeordnet

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Antworten enthalten verbindlichen Operationsbezug, vorhandene Quittung wird idempotent derselben Operation zugeordnet.

**Warum bleibt oder endet der Zustand?** Alle Permutationen ergeben denselben belegten Ausgang ohne neue Wirkung.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021), [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator nutzt eine echte Quittung mehrfach lesend statt mehrfach auslösend.

**Zu prüfen:** Antwortpermutation verändert Zieloperation oder erzeugt zusätzlichen Effekt.

<a id="s057.b02"></a>
### S057.B02 — Ankunftsreihenfolge überschreibt die Anzeige

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Consumer nimmt letzte angekommene Antwort als aktuell ohne Revisionsbindung, Kernjournal ist getrennt erhalten.

**Warum bleibt oder endet der Zustand?** Falsche Beobachtung bleibt bis korrekt korrelierter Vergleich, keine automatische Geschäftsduplikation.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Beabsichtigte Operationen bleiben untersuchbar, der angezeigte Ausgang nicht vertrauenswürdig.

**Zu prüfen:** Consumer sortiert und korreliert zwingend vor jeder relevanten Statusänderung.

<a id="s057.b03"></a>
### S057.B03 — Fehlzugeordnete Antwort löst neue Wirkung aus

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Zusätzliche fehlerhafte Consumeraktion interpretiert verspätete Antwort als Anlass für neuen Dispatch, Teilnehmer nimmt doppelt an.

**Warum bleibt oder endet der Zustand?** Historische Mehrfachwirkung bleibt auch bei später richtiger Reihenfolge.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Erhaltene lokale Versuche helfen Diagnose, nicht gesicherte Wirkungseinmaligkeit.

**Zu prüfen:** Duplikate werden nur gelesen und führen nie zu neuem Dispatch.

**Architekturfolge für diesen Stressor:** Antwortkorrelation mit Operation und Revision vor Zustandsfortschreibung durchsetzen. Doppelte Antworten dürfen keine neuen fachlichen Befehle implizieren.

<a id="s058"></a>
## S058 — Ein nicht-lokaler Listener wird versehentlich öffentlich gebunden

**Ursprung:** operations; A6-Zustände: OP001, OP007, OP027. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s058.b01"></a>
### S058.B01 — Öffentlich erreichbar, ungebundene Aufrufe bleiben ausgeschlossen

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Listener öffentlich, aber echte Principal-/Capability-Grenze vor Datenabgabe und Mutation intakt.

**Warum bleibt oder endet der Zustand?** Reichweite bleibt bis Konfigurationsänderung erweitert, zugelassene Arbeit kann weitergehen.

**Zugeordnete Residues:** [OPR025: Gebundene Aufruferautorität](residues.md#opr025)

**Was bleibt warum nutzbar?** Kernel kann zulässige Aufrufe weiterhin von fremden unterscheiden.

**Zu prüfen:** Anonymer oder nur mit fremder Session-ID auftretender Client erhält geschützte Daten.

<a id="s058.b02"></a>
### S058.B02 — Öffentliche Reichweite trifft ausnutzbare Autoritätslücke

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Unbefugter nutzt tatsächlichen Authentifizierungsfehler und liest Daten oder löst nicht erlaubte Mutation aus.

**Warum bleibt oder endet der Zustand?** Offenlegung und konkrete unautorisierte Wirkung bleiben, spätere Listenerkorrektur macht sie nicht ungeschehen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für bereits verlorene Vertraulichkeit beziehungsweise Zustimmungstreue keines; übrige Systemfähigkeiten sind nicht pauschal verloren.

**Zu prüfen:** Keine unbefugte Kenntnis oder Wirkung erfolgte und die Grenze weist alle solchen Aufrufe zurück.

**Architekturfolge für diesen Stressor:** Nichtlokale Listeneraktivierung explizit und Principalbindung unabhängig von Netzwerkreichweite prüfen. Öffentlicher Bind ist vergrößerte Angriffsfläche, nicht automatisch Offenlegung.

<a id="s059"></a>
## S059 — Tailscale und SSH fallen gemeinsam aus während Operator fern ist

**Ursprung:** operations; A6-Zustände: OP033, OP001, OP031. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s059.b01"></a>
### S059.B01 — Beide Fernwege weg, lokale Arbeit lebt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Tailscale und SSH sind einzige Wege des entfernten Operators, lokale Maschine und bereits autorisierte Aufgaben gesund.

**Warum bleibt oder endet der Zustand?** Fernkontrolle fehlt solange gemeinsamer Zugangsausfall anhält, lokale Arbeit kann weiter Fakten erzeugen.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001), [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Lokaler Dispatcher nutzt erhaltene Aufträge, entfernter Operator hat dadurch noch keinen Livezugang.

**Zu prüfen:** Lokale Arbeit benötigt zwingend einen der ausgefallenen Fernwege.

<a id="s059.b02"></a>
### S059.B02 — Tatsächlich erreichbarer lokaler Vertreter kann prüfen

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetenter befugter Mensch vor Ort hat unabhängig nutzbaren lokalen IPC und verständlichen Fallstand.

**Warum bleibt oder endet der Zustand?** Kontrolllücke endet für den konkreten Fall nach frischer Prüfung, nicht allein durch Existenz des Sockets.

**Zugeordnete Residues:** [OPR009: Unabhängiger lokaler Bedienpfad](residues.md#opr009), [OPR032: Benutzbarer Entscheidungsübergabestand](residues.md#opr032)

**Was bleibt warum nutzbar?** Vertreter kann lokale Evidenz lesen und im Rahmen seiner Rechte entscheiden.

**Zu prüfen:** Vertreter fehlt Zugriff oder benötigt exklusives Wissen des unerreichbaren Owners.

<a id="s059.b03"></a>
### S059.B03 — Kein erreichbarer Mensch für neue Entscheidung

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur entfernter Operator darf entscheiden, beide Wege bleiben weg, Taskkern bewahrt Bedarf.

**Warum bleibt oder endet der Zustand?** Human-gated Arbeit wartet, obwohl andere lokale Arbeit laufen kann.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Bedarf bleibt gespeichert, rechtzeitige menschliche Eingriffsfähigkeit nicht.

**Zu prüfen:** Eine erlaubte erreichbare Vertretung ist tatsächlich handlungsfähig.

**Architekturfolge für diesen Stressor:** Fernzugang und lokale Ausführung getrennt darstellen. Bundled IPC hilft nur einer tatsächlich am Gerät berechtigten erreichbaren Person.

<a id="s060"></a>
## S060 — Teilnehmer drosselt alle Anfragen über Tage mit Rate Limit

**Ursprung:** operations; A6-Zustände: OP020, OP003, OP001, OP004. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s060.b01"></a>
### S060.B01 — Feste Quote hält begrenzte Originalqueue zurück

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Teilnehmer drosselt unabhängig von Retrylast, Zulauf über akzeptiertem Dienst, Aufnahmebudget intakt.

**Warum bleibt oder endet der Zustand?** Quota hält Warten aufrecht; nach Aufhebung und ausreichendem Dienst kann endlicher Vorrat drainen.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Scheduler behält zugelassene Originalarbeit, nicht Fristtreue oder unbegrenzte Aufnahme.

**Zu prüfen:** Zugelassene Queue geht verloren oder Drosselung hängt tatsächlich von Retrylast ab.

<a id="s060.b02"></a>
### S060.B02 — Retrylast erhält sich über adaptive Drosselung

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Mehr Retries verschärfen Quote oder Latenz, dies erzeugt weitere Retries; begrenztes Hochlastregime nach initialer Drosselung bei gleichem Grundbedarf.

**Warum bleibt oder endet der Zustand?** Retrylast → strengere Drosselung → längere Wartezeit → Retrylast. Feste Quote ohne diese Kante wäre vorheriger Zweig.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskfakten bleiben bei intaktem Store untersuchbar, Teilnehmerfortschritt nicht garantiert.

**Zu prüfen:** Bei gleicher Basispolicy und Grundlast nach Schockende unterscheiden sich hohe und niedrige Anfangslast nicht oder Retries verändern Quote nicht.

<a id="s060.b03"></a>
### S060.B03 — Begrenzter Restvorrat nach Quotenende

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Quote wieder ausreichend, gültige endliche Originalqueue ohne weitere Reproduktion erhalten.

**Warum bleibt oder endet der Zustand?** Dienst über Zulauf beendet Rückstand; abgelaufene Arbeit braucht eigene Disposition.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Scheduler kann gültige ursprüngliche Aufträge abarbeiten.

**Zu prüfen:** Trotz positiver Dienstreserve und fehlender Reproduktion wächst der Rückstand.

**Architekturfolge für diesen Stressor:** Rate-Limit-Vertrag und Retryrückwirkung messen. Dauernde 429 unter fester Quote sind äußere Begrenzung, nicht allein ein selbsttragender Attraktor.

<a id="s061"></a>
## S061 — Registry benennt Zielagent um nachdem Task queued wurde

**Ursprung:** operations; A6-Zustände: OP001, OP026, OP031, OP025. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s061.b01"></a>
### S061.B01 — Umbenennung erhält gebundene Zielidentität

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Task bindet stabile Ziel-ID, erhaltene historische Revision und geprüfte neue Namenszuordnung betreffen denselben legitimen Agentbezug.

**Warum bleibt oder endet der Zustand?** Nach Auflösung bleibt Originalaufgabe routbar ohne neue Zielabsicht.

**Zugeordnete Residues:** [OPR026: Historische Scope-Identität mit Lebenszyklus](residues.md#opr026), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Assignment-Bearbeiter kann alte und neue Bezeichnung auf dasselbe erlaubte Ziel beziehen.

**Zu prüfen:** Neuer Inhaber des alten Namens erhält Task ohne explizite Neuzuordnung.

<a id="s061.b02"></a>
### S061.B02 — Alte Zielidentität derzeit nicht routbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Historisches Ziel erhalten, aktuelle Registry hat keinen zulässigen zugehörigen Empfänger.

**Warum bleibt oder endet der Zustand?** Warten auf erlaubte explizite Zuordnung statt stillen Namenersatz.

**Zugeordnete Residues:** [OPR026: Historische Scope-Identität mit Lebenszyklus](residues.md#opr026)

**Was bleibt warum nutzbar?** Operator kann erkennen welches Ziel ursprünglich gemeint war, nicht automatisch an anderen Agent liefern.

**Zu prüfen:** Assignment löst bloßen alten Namen auf anderen Agent auf.

<a id="s061.b03"></a>
### S061.B03 — Späte Namensauflösung trifft falschen Agent

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur veränderlicher Name als Auswahl, andere Identität erhält Task, originaler Taskkern bleibt unabhängig erhalten.

**Warum bleibt oder endet der Zustand?** Fehlattribution bleibt bis Klärung, tatsächliche Offenlegung hängt von geliefertem Inhalt und Rechten ab.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskabsicht bleibt nachlesbar, richtige Empfängerbindung ist nicht erhalten.

**Zu prüfen:** Nachgewiesene verbindliche Stable-ID-Prüfung verhindert die Umleitung.

**Architekturfolge für diesen Stressor:** Dauerhafte Zielidentität von späterer agent-name-Auswahl trennen. Rename darf keinen stillen Ersatzempfänger autorisieren, historische Registryrevision bleibt Teil der erklärbaren Absicht.

<a id="s062"></a>
## S062 — Scope-Verzeichnis wird gelöscht und am selben Pfad ersetzt

**Ursprung:** operations; A6-Zustände: OP025, OP026, OP015, OP012. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s062.b01"></a>
### S062.B01 — Ersatzverzeichnis wird vor Nutzung erkannt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Ursprüngliche Scopehistorie und objektgebundene Zugriffsgrenze vergleichen tatsächlichen Ersatz vor betroffener Factory-Nutzung.

**Warum bleibt oder endet der Zustand?** Bis explizite rechtmäßige Neuzuordnung kein Zugriff unter alter Identität.

**Zugeordnete Residues:** [OPR026: Historische Scope-Identität mit Lebenszyklus](residues.md#opr026), [OPR016: Objektgebundener Dateizugang](residues.md#opr016)

**Was bleibt warum nutzbar?** Operator kann alte Identität sehen und Factory-Schreiber bleibt am erlaubten geprüften Objekt begrenzt.

**Zu prüfen:** Neues gleichnamiges Verzeichnis wird ohne Entscheidung als altes angenommen.

<a id="s062.b02"></a>
### S062.B02 — Gleicher Pfad verdeckt anderes Objekt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur Pfadexistenz wird geprüft, neue Inhalte unter altem Scope interpretiert, Taskkern getrennt intakt.

**Warum bleibt oder endet der Zustand?** Falsche Zuordnung bleibt bis Identitätsvergleich, keine notwendige Rückkopplung.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Alte Auftragsfakten können bleiben, Verzeichnis- und Artefaktkontinuität fehlen.

**Zu prüfen:** Persistierte Originalidentität wird zwingend gegen Ersatz verglichen.

<a id="s062.b03"></a>
### S062.B03 — Alte einzigartige Workspacebytes mit Verzeichnis weg

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Gelöschtes Verzeichnis enthielt einzige benötigte Artefaktbytes und keine andere Quelle ist erreichbar.

**Warum bleibt oder endet der Zustand?** Neuer Inhalt gleichen Pfads rekonstruiert alte Bytes nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für exakte Originalbytes keines; neue Pfadexistenz ist keine Erhaltung.

**Zu prüfen:** Unverändertes Originalobjekt oder vollständige unabhängige Version wird gefunden.

**Architekturfolge für diesen Stressor:** Scope-ID und ursprüngliches Verzeichnisobjekt getrennt vom Pfad speichern. Existenzprüfung und heutiger FileId genügen nicht zum Beweis historischer Kontinuität.

<a id="s063"></a>
## S063 — Symlink ändert Ziel zwischen Validierung und Dateischreibzugriff

**Ursprung:** operations; A6-Zustände: OP025, OP027, OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s063.b01"></a>
### S063.B01 — Zugriff bleibt trotz Symlinktausch am geprüften Objekt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Handle-/no-follow-Vertrag auf dem tatsächlichen Schreibpfad durchgesetzt, ursprüngliches Objekt lag in erlaubter .factory-Wurzel.

**Warum bleibt oder endet der Zustand?** Schreibzugriff erreicht nur dieses Objekt oder endet mit Fehler, nicht unbemerkt neues Ziel.

**Zugeordnete Residues:** [OPR016: Objektgebundener Dateizugang](residues.md#opr016)

**Was bleibt warum nutzbar?** Factory-Schreiber kann Zielbegrenzung behalten; erfolgreicher fachlicher Inhalt ist damit nicht bewiesen.

**Zu prüfen:** Deterministischer Tausch lenkt Schreibwirkung außerhalb der geprüften erlaubten Wurzel.

<a id="s063.b02"></a>
### S063.B02 — Späte Pfadauflösung schreibt auf anderes Ziel

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nach Prüfung wird Symlink neu verfolgt und betroffener Schreibzugriff erreicht anderes unerlaubtes Objekt.

**Warum bleibt oder endet der Zustand?** Falsche Schreibwirkung bleibt bis spezifischer Korrektur, keine Selbstrückkehr nötig.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Unbetroffener Taskkern kann beabsichtigtes Ziel belegen, tatsächliche Schreibgrenze wurde verletzt.

**Zu prüfen:** Tausch führt nur zum I/O-Fehler oder Zugriff bleibt am geprüften Objekt.

**Architekturfolge für diesen Stressor:** Factory-Dateischreiben über objektgebundene Zugriffe nur innerhalb geprüfter .factory-Wurzeln konkretisieren; vorherige canonicalize-Prüfung allein ist keine TOCTOU-Garantie.

<a id="s064"></a>
## S064 — Parent-Scope verschiebt sich und ändert Delegationsrechte

**Ursprung:** operations; A6-Zustände: OP026, OP031, OP010, OP025. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s064.b01"></a>
### S064.B01 — Neue Delegation wird nach aktuellen Rechten verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alte Scope-/Taskhistorie erhalten, Move ändert aktuelle Berechtigung und künftiger Aufruf prüft diese Revision.

**Warum bleibt oder endet der Zustand?** Auftrag wartet auf erlaubte neue Route oder Entscheidung, alte zulässige Annahme bleibt historische Tatsache.

**Zugeordnete Residues:** [OPR026: Historische Scope-Identität mit Lebenszyklus](residues.md#opr026), [OPR030: Erhaltene Delegationskette](residues.md#opr030)

**Was bleibt warum nutzbar?** Delegationsprüfer kann alte Kette und heutige Grenze getrennt nutzen.

**Zu prüfen:** Move verändert alte Herkunftskette oder neue Delegation benutzt still alte Rechte.

<a id="s064.b02"></a>
### S064.B02 — Heutige Abstammung schreibt alte Bedeutung um

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Historisches Replay liest neue Parentbeziehung, rohe ursprüngliche Events sind getrennt erhalten aber benötigte damalige Relation möglicherweise nicht.

**Warum bleibt oder endet der Zustand?** Gleiche Rohdaten ergeben andere historische Rechte bis korrekter historischer Bezug vorhanden ist.

**Zugeordnete Residues:** [OPR004: Unveränderte kanonische Eingabebytes](residues.md#opr004)

**Was bleibt warum nutzbar?** Replaybearbeiter kann Originaleingaben prüfen, nicht nie aufgezeichnete Rechte erfinden.

**Zu prüfen:** Replay bleibt bei geändertem Parent invariant und damals relevante Relation ist authentisch erhalten.

**Architekturfolge für diesen Stressor:** Historische Annahmerechte und Rechte zukünftiger Delegation getrennt an Registryrevisionen binden. Ein Move darf alte Auditfakten nicht nach heutiger Verwandtschaft neu bewerten.

<a id="s065"></a>
## S065 — Zwei Klone derselben Installation starten auf getrennten Macs

**Ursprung:** operations; A6-Zustände: OP023, OP025, OP016, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s065.b01"></a>
### S065.B01 — Zweiter Clone ist wirklich nur lesend oder gefenced

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Externe Generation ist auf allen Effektpfaden verpflichtend oder Clone besitzt tatsächlich keine Dispatchautorität; kopierte Daten lesbar.

**Warum bleibt oder endet der Zustand?** Nur autorisierte Generation kann neue Wirkungen annehmen lassen; Clone kann Inventur leisten.

**Zugeordnete Residues:** [OPR027: Exklusive Dispatch-Generation](residues.md#opr027), [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Wiederhersteller liest Snapshot ohne aus kopierter queued-Historie Doppelwirkungen abzuleiten.

**Zu prüfen:** Beide Kopien können beim Fake-Teilnehmer gleichzeitig unter derselben Autorität neue Wirkung annehmen lassen.

<a id="s065.b02"></a>
### S065.B02 — Beide Clones halten sich lokal für exklusiv

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Jeder Store vergibt eigene Leases, beide dispatchen kopierte Arbeit und Teilnehmer akzeptiert beide.

**Warum bleibt oder endet der Zustand?** Doppelte Effekte und auseinanderlaufende Historien bleiben auch nach Stop eines Clones.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Jeweilige lokale Taskfakten sind noch lesbar, globaler Einmaligkeits- und Identitätsanspruch nicht erhalten.

**Zu prüfen:** Unabhängig wirksames Fence oder Teilnehmer-Einmaligkeit verhindert zweite Annahme.

<a id="s065.b03"></a>
### S065.B03 — Zusätzlich gemeinsamer Workspace mit zwei Schreibern

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Clones haben Zugriff auf denselben Workspace und starten beide tatsächlich schreibfähige Prozesse.

**Warum bleibt oder endet der Zustand?** Konflikt dauert mit Schreibfähigkeiten, kopierte Identitäten allein erzeugen ihn nicht.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Nicht betroffene Taskdaten bleiben nutzbar, exklusive Dateiänderungsattribution fehlt.

**Zu prüfen:** Workspaces sind physisch getrennt oder nur ein Clone besitzt Schreibrechte.

**Architekturfolge für diesen Stressor:** Cross-Clone-Dispatchautorität außerhalb kopierter SQLite-Leases prüfen oder Clone technisch ohne Sendefähigkeit halten. Kein zweiter lokaler aktiv-Schalter als globales Fence.

<a id="s066"></a>
## S066 — Client zeigt legitime Session-ID eines anderen Agents vor

**Ursprung:** operations; A6-Zustände: OP026, OP007, OP025. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s066.b01"></a>
### S066.B01 — Geliehene Session-ID verleiht keine Autorität

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Transport bindet echten Principal und Session/Scope-Rechte, fremde gültige Session-ID passt nicht dazu.

**Warum bleibt oder endet der Zustand?** Ungültiger Aufruf endet vor Mutation oder Datenfreigabe, bestehende Arbeit bleibt.

**Zugeordnete Residues:** [OPR025: Gebundene Aufruferautorität](residues.md#opr025)

**Was bleibt warum nutzbar?** Kernel kann berechtigte Agentaufträge weiter unterscheiden und fremden Aufruf abweisen.

**Zu prüfen:** Geliehene ID allein erlaubt Lesen oder Ändern in der fremden Session.

<a id="s066.b02"></a>
### S066.B02 — Identifierexistenz ersetzt fälschlich Authentifizierung

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Aufrufer kann fremde Session-ID ohne Besitzbeleg verwenden und erhält tatsächlich fremde Rechte.

**Warum bleibt oder endet der Zustand?** Unberechtigte Zugriffsfähigkeit hält bis Bindung/Widerruf greift, bereits erfolgte Kenntnis ist nicht rücknehmbar.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die verletzte Principalgrenze und bereits offengelegte Information keines; keine unabhängige Vertrauenswurzel wird unterstellt.

**Zu prüfen:** Obligatorischer Transportkontext weist fremden Principal vor jeder geschützten Nutzung zurück.

**Architekturfolge für diesen Stressor:** Sender-Kontext vor Delegations-/Taskprüfung authentifizieren. Ein existierender Session-Identifier ist öffentliche Identität und keine Besitzberechtigung.

<a id="s067"></a>
## S067 — Task delegiert A nach B nach C nach A

**Ursprung:** operations; A6-Zustände: OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s067.b01"></a>
### S067.B01 — Rückkehr von C nach bereits besuchtem A verweigert

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** A-B-C-Kette unverändert und authentifiziert im kontrollierten Delegationspfad, Zielregistrierung geprüft.

**Warum bleibt oder endet der Zustand?** Request endet vor neuer Taskanlage, vorhandene Aufgaben bleiben getrennt bearbeitbar.

**Zugeordnete Residues:** [OPR030: Erhaltene Delegationskette](residues.md#opr030), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Prüfer kann aus bestehender Provenienz die Wiederholung erkennen ohne bestehende Arbeit zu löschen.

**Zu prüfen:** Legitimer Delegationsaufruf mit erhaltenem A in Kette legt neuen Task an A an.

**Architekturfolge für diesen Stressor:** Erhaltene Delegationskette als Aufnahmegrenze testen. Sie begrenzt Rückkehr in derselben Kette, nicht frische eigenständige Taskproduzenten.

<a id="s068"></a>
## S068 — Agent erzeugt statt Weiterdelegation immer neue identische Tasks

**Ursprung:** operations; A6-Zustände: OP004, OP023, OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s068.b01"></a>
### S068.B01 — Endliche Zusatzaufgaben ohne Selbstreproduktion

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Einmaliger Produzent erzeugt endliche Menge, danach keine neue Erzeugung aus Arbeit oder Fehlern; Originalqueue begrenzt und gültig.

**Warum bleibt oder endet der Zustand?** Bei Dienst über weiterem Zulauf drainen zugelassene Aufgaben, überzählige Arbeit kann trotzdem teuer sein.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Scheduler behält zugelassene Aufträge, nicht pauschal Nutzen jeder semantisch gleichen Kopie.

**Zu prüfen:** Nach Seedende erzeugen Aufgaben selbst weiter neue Wurzeln.

<a id="s068.b02"></a>
### S068.B02 — Neue Tasks erhalten begrenztes Hochlastregime

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Fresh-root-Recht erlaubt Kopien, Last senkt Abschlüsse und fehlender Fortschritt veranlasst Agent zu weiteren Kopien; nach Seed besteht bei gleichem Grundbedarf begrenzte wiederkehrende Hochlast.

**Warum bleibt oder endet der Zustand?** Rückstau → scheinbar fehlender Fortschritt → neue Tasks → mehr Last → Rückstau. Ein großer endlicher Baum reicht nicht.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Bei lesbarem Store bleiben ursprüngliche Taskfakten untersuchbar, produktiver Fortschritt nicht.

**Zu prüfen:** Bei gleichem Grundbedarf nach Seedende verschwindet zusätzliche Erzeugung oder keine Last-zu-Neuanlage-Kante existiert.

<a id="s068.b03"></a>
### S068.B03 — Ungebremste Vermehrung erschöpft Aufnahmekapazität

**Art:** eskalation. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Jede Aufgabe erzeugt dauerhaft mehr neue Aufgaben als Abschlüsse entfernen, kein wirksames Budget und kein begrenztes Hochlastregime.

**Warum bleibt oder endet der Zustand?** Menge wächst bis Ressourcenende statt zu einem belegten Attraktor zu konvergieren.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Unter ungebremster Store-Erschöpfung ist keine dauerhaft nutzbare Queue garantiert; bis dahin vorhandene Records sind kein Erhaltungsbeweis.

**Zu prüfen:** Erzeugung ist endlich oder wirksames Budget bewahrt zugängliche Originalqueue.

**Architekturfolge für diesen Stressor:** Fresh-root-Erzeugungsrechte und Aufnahmebudget getrennt von endlicher Delegationskette begrenzen. Inhaltsgleichheit allein definiert keine fachliche Duplikation.

<a id="s069"></a>
## S069 — Scope wird stillgelegt während alte Tasks und Memory existieren

**Ursprung:** operations; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s069.b01"></a>
### S069.B01 — Scope stillgelegt, erlaubte Historie bleibt lesbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Expliziter Tombstone, keine neue Aufnahme, berechtigter Historienzugang und Task-/Referenzdisposition dauerhaft festgehalten.

**Warum bleibt oder endet der Zustand?** Lokale neue Arbeit bleibt untersagt, zulässige Historienprüfung bleibt bis definierter Retention/Autoritätsgrenze möglich.

**Zugeordnete Residues:** [OPR026: Historische Scope-Identität mit Lebenszyklus](residues.md#opr026), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Berechtigter Bearbeiter kann alte Identität und Taskstand nutzen; Memorybytes nur wenn tatsächlich erhalten und erlaubt.

**Zu prüfen:** Alte ID wird neu vergeben oder Historienlesen erbt still Rechte eines neuen Pfadinhabers.

<a id="s069.b02"></a>
### S069.B02 — Lokale Stilllegung mit offenen Außenpflichten

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Scope nicht mehr aktiv, erhaltenes Intentjournal enthält noch möglicherweise angenommene Teilnehmeroperationen.

**Warum bleibt oder endet der Zustand?** Bis erlaubter Nachbeobachtung und Quittung beziehungsweise bewusster Disposition bleiben konkrete Pflichten offen.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Zuständiger Nachfolger kann bekannte offene Operationen verfolgen; kein Schluss auf keine unbekannten späten Wirkungen.

**Zu prüfen:** Stilllegung markiert alle Teilnehmerwirkungen automatisch abgeschlossen oder löscht nötige Operationsbezüge.

<a id="s069.b03"></a>
### S069.B03 — Erforderlicher alter Inhalt nach rechtmäßiger Löschung weg

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Stilllegungsvertrag verlangt Löschung bestimmter einzig vorhandener Memory-/Artefaktbytes, keine zulässige Rekonstruktionsquelle bleibt.

**Warum bleibt oder endet der Zustand?** Für exakt diese Inhalte bleibt Nutzbarkeit nach Löschung weg, auch wenn Identität erhalten bleibt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Keines für den gelöschten Inhalt; unerlaubte Schattenkopie wird nicht als Residue vorgeschlagen.

**Zu prüfen:** Eine rechtmäßig gehaltene vollständige Quelle bleibt gemäß tatsächlichem Vertrag verfügbar.

<a id="s069.b04"></a>
### S069.B04 — Bedeutung von stillgelegt nicht definiert

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Keine festgelegte Taskdisposition, Retention, Leseberechtigung oder Nachpflichtverantwortung.

**Warum bleibt oder endet der Zustand?** Aus Verzeichnislöschung oder fehlender Registrierung allein folgt kein geschlossener Lebenszyklus.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Es ist kein bestimmter erlaubter Restzugriff belegt; Produktvertrag muss vor Nutzung entschieden werden.

**Zu prüfen:** Expliziter Stilllegungsvertrag entscheidet alle hier betroffenen Grenzen.

**Architekturfolge für diesen Stressor:** Stilllegungsvertrag muss neue Aufnahme, alte Tasks, erlaubte Memory-/Historienzugriffe und externe Restpflichten getrennt entscheiden. Tombstone ist keine Teilnehmerlöschung.

<a id="s070"></a>
## S070 — Case-sensitive Export wird auf case-insensitive Dateisystem restauriert

**Ursprung:** operations; A6-Zustände: OP026, OP012, OP025, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s070.b01"></a>
### S070.B01 — Kollidierender Restore stoppt bei erhaltenem Export

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Export enthält A und a als verschiedene Objekte, Import prüft Kollision vor Schreiben und Originalexport liegt unabhängig lesbar vor.

**Warum bleibt oder endet der Zustand?** Bis verlustfreie explizite Abbildung vorliegt kein zerstörender Import.

**Zugeordnete Residues:** [OPR008: Unabhängig lesbarer Export](residues.md#opr008)

**Was bleibt warum nutzbar?** Wiederhersteller kann beide Originalinhalte aus Export lesen obwohl Zielfilesystem sie nicht gleichnamig tragen kann.

**Zu prüfen:** Import überschreibt Export oder verliert ein Objekt vor Kollisionsmeldung.

<a id="s070.b02"></a>
### S070.B02 — Verlustfreie Abbildung erhält getrennte Originalobjekte

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Import definiert nachvollziehbare eindeutige Objektabbildung unter .factory, beide ursprünglichen Byteobjekte bleiben getrennt referenziert.

**Warum bleibt oder endet der Zustand?** Nach Prüfung kann auf beide Inhalte ohne Namensgleichsetzung zugegriffen werden.

**Zugeordnete Residues:** [OPR005: Erhaltenes originales Artefaktobjekt](residues.md#opr005)

**Was bleibt warum nutzbar?** Ergebnisnutzer erhält beide Originalbytes, bequeme Pfadschreibweise kann verändert sein.

**Zu prüfen:** Eine Referenz liefert die Bytes des anderen Case-Objekts.

<a id="s070.b03"></a>
### S070.B03 — Stilles Überschreiben löscht einziges Case-Objekt

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Case-kollidierender Import überschreibt einzigartige Bytes und Originalexport ist nicht mehr zugänglich.

**Warum bleibt oder endet der Zustand?** Gleicher Zielname stellt zerstörte Variante nicht wieder her.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die verlorene Variante keines; ein Alias ist keine zweite Originalkopie.

**Zu prüfen:** Unveränderter Export oder andere Originalquelle enthält beide Varianten.

**Architekturfolge für diesen Stressor:** Case-Kollision vor Restore-Schreiben erkennen und Originalexport unverändert halten. Laufzeit-FileId-Aliasprüfung ersetzt keine verlustfreie portable Namensabbildung.

<a id="s071"></a>
## S071 — CLI wiederholt create nach verlorener Antwort

**Ursprung:** operations; A6-Zustände: OP026, OP023, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s071.b01"></a>
### S071.B01 — Gleicher Commandkey liefert aufgezeichnetes Ergebnis

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Client behält gleichen principalgebundenen Intentkey und Payload, Mutation und Ergebnis wurden atomar gespeichert.

**Warum bleibt oder endet der Zustand?** Verlorene Antwort wird durch Wiederlesen ersetzt, keine zweite Taskanlage.

**Zugeordnete Residues:** [OPR028: Dauerhaftes Kommandoergebnis](residues.md#opr028)

**Was bleibt warum nutzbar?** CLI erhält ursprüngliche Task-ID und Commandausgang trotz Transportverlust.

**Zu prüfen:** Retry erzeugt zweite Task-ID oder gleicher Key mit anderem Payload wird still gleichgesetzt.

<a id="s071.b02"></a>
### S071.B02 — Nur gleicher Task-Primärschlüssel kollidiert

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Aufruf benutzt historischen C6-Insert mit gleicher UUID, erste Taskrow ist dauerhaft vorhanden.

**Warum bleibt oder endet der Zustand?** Zweiter Insert endet mit Fehler, ursprünglicher Task bleibt; Fehler ist nicht gespeichertes Ergebnis.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Operator kann vorhandenen Task nach ID suchen, CLI hat nicht automatisch eine erfolgreiche idempotente Antwort.

**Zu prüfen:** Helper legt trotz gleicher UUID zweite Taskrow an.

<a id="s071.b03"></a>
### S071.B03 — Neue Retry-ID erzeugt zweite Aufgabe

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Antwort verloren, Client erstellt mit neuer ID dieselbe beabsichtigte Aufgabe ohne logischen Commandbezug.

**Warum bleibt oder endet der Zustand?** Zwei lokale Aufgaben bleiben; doppelte Außenwirkung folgt erst mit weiteren Dispatches.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Beide Taskfakten können untersucht werden, einmalige Aufnahme der Absicht ist verloren.

**Zu prüfen:** Commandebene führt beide Versuche auf dieselbe Intention und Task-ID zurück.

**Architekturfolge für diesen Stressor:** Logische API-Idempotenz mit dauerhaftem gebundenem Ergebnis anbieten; aktueller create-Helper mit UUID-Insertkollision ist nur eine engere Ablehnung, keine Retry-Ergebnisgarantie.

<a id="s072"></a>
## S072 — Cancel trifft nach Zustellcommit aber vor Writer-Aufruf ein

**Ursprung:** operations; A6-Zustände: OP035, OP024, OP028, OP026. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s072.b01"></a>
### S072.B01 — Cancel vor wirksamem Zustellversuch endet ohne Writer

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Cancel ist vor initialer Zustellprüfung committet und C1 prüft terminalen Status.

**Warum bleibt oder endet der Zustand?** Zustellrequest wird verweigert, Taskstorno bleibt; daraus folgt kein allgemeiner Stopp früherer unabhängiger Wirkungen.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Operator besitzt gültigen Cancel-Taskstand für diesen nie begonnenen Zustellpfad.

**Zu prüfen:** Writer wird auf diesem Pfad trotz voriger terminaler Prüfung gerufen.

<a id="s072.b02"></a>
### S072.B02 — Cancel nach Intent, späterer Writer bleibt möglich

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** C1-Intent committet, C6-queued-Cancel interleaved vor Writer und kein Callerlock schließt Lücke; beide Records dauerhaft.

**Warum bleibt oder endet der Zustand?** Terminalstatus verhindert den schon vorbereiteten Writer nicht. Tatsächlicher Empfang braucht zusätzliche Übergabeaktion und Evidenz.

**Zugeordnete Residues:** [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Operator kann Versuch und Cancel lesen, nicht daraus beweisen dass niemand einen Prompt erhielt.

**Zu prüfen:** Obligatorischer Callerlock oder gleichwertige Dispatchgrenze verhindert Writer im genauen Interleaving.

<a id="s072.b03"></a>
### S072.B03 — Nach Cancel tatsächlich weitergegebene Wirkung ist belegt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Zusätzliche reale Übergabe und Workeraktion erfolgen nach Writer, irreversible Teilnehmerannahme ist verbindlich quittiert.

**Warum bleibt oder endet der Zustand?** Historische Wirkung bleibt trotz lokaler Cancelrow wahr, Consent-Verstoß hängt vom tatsächlichen Vertrag ab.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann genau angenommene Wirkung belegen; lokale Stornierung ist kein Unsenden.

**Zu prüfen:** Es gab nur gerenderten Prompt ohne Übergabe oder keine verbindliche Teilnehmerannahme.

**Architekturfolge für diesen Stressor:** Cancel- und Writer-Schnittpunkt im Caller serialisieren oder expliziten Ausgangskonflikt führen. Keine atomare Fernstornierung oder externe Annahme aus manuellem Writer ableiten.

<a id="s073"></a>
## S073 — Zwei Assignments sehen denselben idle Worker

**Ursprung:** operations; A6-Zustände: OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s073.b01"></a>
### S073.B01 — Nur erstes konkurrierendes Assignment belegt Worker

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Assignments benutzen C3 und denselben Store, erstes erzeugt nichtterminale Zuordnung.

**Warum bleibt oder endet der Zustand?** Zweiter Aufruf sieht Belegung und endet ohne zweite Zuteilung; erster Task muss noch bearbeitet werden.

**Zugeordnete Residues:** [OPR029: Transaktionale Worker-Belegung](residues.md#opr029)

**Was bleibt warum nutzbar?** Assignment-Entscheider behält eine eindeutige verwaltete Belegung.

**Zu prüfen:** Zwei C3-Aufrufe weisen gleichzeitig zwei nichtterminale Tasks derselben Session zu.

**Architekturfolge für diesen Stressor:** Idle-Prädikat und Assignment in einer Storetransaktion bewahren. Queued und blocked zählen belegt, nicht erst running.

<a id="s074"></a>
## S074 — Blocked Task wird requeued ohne neue Zustellautorisierung

**Ursprung:** operations; A6-Zustände: OP014, OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s074.b01"></a>
### S074.B01 — Verbrauchter Versuch bleibt nach Requeue gesperrt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Mindestens ein gezählter Versuch verbraucht unverändertes Budget, C1 wird verwendet.

**Warum bleibt oder endet der Zustand?** Requeue allein ändert Autorisierung nicht, weiterer Writer bleibt aus.

**Zugeordnete Residues:** [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002)

**Was bleibt warum nutzbar?** Operator kann alten Versuch und fehlende neue Erlaubnis prüfen.

**Zu prüfen:** Writer wird trotz unverändert verbrauchtem Budget erneut gerufen.

<a id="s074.b02"></a>
### S074.B02 — Blocked vor erstem Versuch hat noch Originalbudget

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Noch kein Versuch verbraucht, ursprüngliche Zustellautorisierung gültig, übrige Guards erfüllt.

**Warum bleibt oder endet der Zustand?** Erster tatsächlicher Versuch verbraucht Originalbudget; blocked allein war keine Lieferung.

**Zugeordnete Residues:** [KOR001: Verbleibende Zustellautorisierung](residues.md#kor001), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Der erhaltene Task und seine unverbrauchte gültige Zustellrevision erlauben die ursprüngliche begrenzte Aufnahme. Es gibt ausdrücklich noch keinen historischen Versuchsrecord und daher kein OPR002.

**Zu prüfen:** Requeue wird allein wegen blocked als bereits zugestellt behandelt oder erster Versuch verbraucht Budget nicht.

**Nachtrag des Koordinators:** [A7S02](../review-dispositions.md#a7s02); ursprüngliche Abgabe unverändert.

<a id="s074.b03"></a>
### S074.B03 — Bewusste neue Autorisierung ändert nur Zustellbudget

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Mindestens ein früherer Versuch ist dauerhaft belegt und seine damalige Autorisierung verbraucht. Nach Einsicht entscheidet ein berechtigter Operator über eine zusätzliche Zustellrevision. Historie bleibt unverändert und neuer Rest wird atomar mit der neuen Entscheidung erhalten; übrige Guards gelten weiter.

**Warum bleibt oder endet der Zustand?** Neuer Versuch kann beginnen; frühere Außenwirkung oder alte Workspacelease bleiben eigenständig zu prüfen.

**Zugeordnete Residues:** [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002), [KOR001: Verbleibende Zustellautorisierung](residues.md#kor001)

**Was bleibt warum nutzbar?** OPR002 erhält die frühere Versuchsevidenz und deren verbrauchte Erlaubnis. KOR001 bezeichnet ausschließlich den zusätzlichen gültigen Rest der neuen Revision. Keines klärt frühere Außenwirkung oder Workspacebesitz.

**Zu prüfen:** Neue Autorisierung löscht alte Versuche oder gibt unklare Lease automatisch frei.

**Nachtrag des Koordinators:** [A7S02](../review-dispositions.md#a7s02); ursprüngliche Abgabe unverändert.

**Architekturfolge für diesen Stressor:** Requeue nicht mit erneuter Zustellautorisierung gleichsetzen und verbrauchte Versuche nicht durch Fehleroutcome oder Statuswechsel entfernen.

<a id="s075"></a>
## S075 — Worker liefert Artefaktpfad der auf später überschreibbare Datei zeigt

**Ursprung:** operations; A6-Zustände: OP012, OP005, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s075.b01"></a>
### S075.B01 — Pfad ändert sich, übernommenes Original bleibt

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Originalbytes vor Completion dauerhaft unveränderlich übernommen, danach Arbeitsdatei überschrieben.

**Warum bleibt oder endet der Zustand?** Leser benutzt stabile Objektreferenz und erhält weiter die abgenommenen Bytes.

**Zugeordnete Residues:** [OPR005: Erhaltenes originales Artefaktobjekt](residues.md#opr005)

**Was bleibt warum nutzbar?** Ergebnisnutzer kann Original gegen spätere Datei unterscheiden.

**Zu prüfen:** Objektreferenz folgt dem veränderlichen Pfad oder liefert neue Bytes.

<a id="s075.b02"></a>
### S075.B02 — Nur Pfad blieb, alte Bytes überschrieben

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Keine Originalübernahme oder unabhängige Kopie, späterer Schreibzugriff ersetzt ursprünglichen Inhalt, Taskrow intakt.

**Warum bleibt oder endet der Zustand?** Alter String bleibt, Originalbytes können daraus nicht rekonstruiert werden.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskbezug bleibt nutzbar, für exaktes ursprüngliches Artefakt keines.

**Zu prüfen:** Tatsächlich erhaltenes Originalobjekt liefert alte Bytes.

<a id="s075.b03"></a>
### S075.B03 — Neue Bytes fälschlich als altes Resultat verwendet

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Downstream liest mutable Path nach Überschreiben und übernimmt neue Bedeutung als geprüfte alte Ausgabe.

**Warum bleibt oder endet der Zustand?** Fehlannahme bleibt bis unabhängiger Abnahmevergleich, kein notwendiger Loop.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskfakten zeigen ursprüngliche Annahmebehauptung, nicht Wahrheit der später gelesenen Ausgabe.

**Zu prüfen:** Consumer verifiziert zwingend ursprünglichen Digest samt eigenständig erhaltenem Objekt.

**Architekturfolge für diesen Stressor:** Immutable Artefaktobjekt bei Abnahme übernehmen statt mutable Path als Originalgarantie behandeln. Aufnahme von Agentoutput und Factory-Schreiben unter .factory getrennt handhaben.

<a id="s076"></a>
## S076 — Taskergebnis enthält mehrere Gigabytes Text

**Ursprung:** operations; A6-Zustände: OP026, OP001, OP034. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s076.b01"></a>
### S076.B01 — Übergroße Completion wird vor Taskmutation abgewiesen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Aufruf erreicht C5-Validierung trotz bereits eventuell großer Callerallokation, alter Store bleibt lesbar.

**Warum bleibt oder endet der Zustand?** Dieser Request endet ohne Statusänderung; Task bleibt offen und Session nicht automatisch frei.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Operator kann bisherigen Taskstand weiter benutzen, kein riesiges Resultat wurde als vollständig akzeptiert.

**Zu prüfen:** Mehrgigabyte-Summary ändert Taskstatus durch complete_with_result.

<a id="s076.b02"></a>
### S076.B02 — Ingressbegrenzung erhält lokalen Betrieb

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Größenprüfung gilt vor unbeschränkter Textallokation und gemeinsame Ressourcenreserve ist wirksam.

**Warum bleibt oder endet der Zustand?** Große Nachricht wird begrenzt abgelehnt, andere Kernelarbeit behält Ressourcen.

**Zugeordnete Residues:** [OPR018: Reservierte unabhängige Hostkapazität](residues.md#opr018)

**Was bleibt warum nutzbar?** Kernel kann weiter nützliche lokale Anfragen bearbeiten statt an Validierungsvorbereitung zu sterben.

**Zu prüfen:** Speicher wächst proportional zum unzulässigen Body bevor Grenze greift.

<a id="s076.b03"></a>
### S076.B03 — Große Bytes separat erhalten, kurze Completion möglich

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Großes zulässiges Artefakt wurde tatsächlich unveränderlich übernommen und Decoder verfügbar; kurze sachlich passende Abnahme vorhanden.

**Warum bleibt oder endet der Zustand?** Geprüfte Referenz kann zusammen mit begrenzter Summary normal committet werden.

**Zugeordnete Residues:** [OPR005: Erhaltenes originales Artefaktobjekt](residues.md#opr005)

**Was bleibt warum nutzbar?** Ergebnisnutzer hat den echten Inhalt, nicht eine abgeschnittene Zeichenfolge.

**Zu prüfen:** Referenz zeigt auf fehlende Datei oder semantisch unbrauchbare unvollständige Bytes.

**Architekturfolge für diesen Stressor:** Resultatgröße vor persistenter Mutation prüfen und zusätzlich Ingressallokation begrenzen. Große Inhalte nur als wirklich übernommene Artefaktobjekte mit kurzer überprüfter Zusammenfassung verwenden.

<a id="s077"></a>
## S077 — Permanent Agent bearbeitet neue Aufgabe während alte auf Permission wartet

**Ursprung:** operations; A6-Zustände: OP026, OP017, OP001, OP016. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s077.b01"></a>
### S077.B01 — Neue Aufgabe wird nicht demselben blocked Worker zugeordnet

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alte nichtterminale Permissionzuordnung erhalten, neuer Aufruf benutzt C3.

**Warum bleibt oder endet der Zustand?** Dieser Worker bleibt belegt bis konkrete alte Aufgabe terminal oder zulässig anders disponiert ist.

**Zugeordnete Residues:** [OPR029: Transaktionale Worker-Belegung](residues.md#opr029)

**Was bleibt warum nutzbar?** Assignment-Bearbeiter behält eindeutige alte Zuständigkeit; andere freie Sessions können separat arbeiten.

**Zu prüfen:** Normaler assign lässt zweite nichtterminale Aufgabe in derselben Session zu.

<a id="s077.b02"></a>
### S077.B02 — Manueller Prompt umgeht Belegungsgrenze

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Mensch oder Harness sendet neue Arbeit direkt während alte Permissionfrage offen ist und beide Kontexte wirken weiter.

**Warum bleibt oder endet der Zustand?** Kontext-/Schreibkonflikt hält mit konkurrierender Aktivität an, permanente Lebensdauer allein erzeugt ihn nicht.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Alte Taskfakten bleiben, verlässliche Zuordnung der neuen unverwalteten Arbeit fehlt.

**Zu prüfen:** Kein direkter Zusatzprompt oder neue Arbeit ist nachweislich separat isoliert.

**Architekturfolge für diesen Stressor:** Permanente Kommunikationslebensdauer nicht als konkurrierende Taskbelegung behandeln. Normale Assignmentgrenze schützt nur kontrollierte Aufträge, nicht direkte Harnessprompts.

<a id="s078"></a>
## S078 — Process-Executor Exitcode null obwohl Output leer ist

**Ursprung:** operations; A6-Zustände: OP026, OP005, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s078.b01"></a>
### S078.B01 — Leere Completion ohne Summary oder Artefakt wird verweigert

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** C5 erhält weder nichtleere Summary noch Artefaktpfad, Taskdaten bisher intakt.

**Warum bleibt oder endet der Zustand?** Request endet ohne Completion, Exit null füllt die fehlenden Resultatfelder nicht.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Offener Task bleibt als Auftrag nutzbar, kein fachlicher Erfolg aus Exitcode behauptet.

**Zu prüfen:** NoResult-Aufruf committet dennoch Erfolg.

<a id="s078.b02"></a>
### S078.B02 — Nichtleere Erfolgsaussage verdeckt fehlenden Pflichtoutput

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Caller liefert Erfolgstext oder Pfad zu leerer Datei, Auftrag verlangte echte Ausgabe und kein unabhängiger Test prüft sie.

**Warum bleibt oder endet der Zustand?** Strukturell akzeptierter Fehler bleibt bis passender Prüfbezug eintrifft.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskrow belegt Behauptung, erforderliche Arbeitsleistung ist nicht erhalten.

**Zu prüfen:** Aufgabenspezifische Abnahme weist fehlenden Output vor Annahme zurück.

<a id="s078.b03"></a>
### S078.B03 — Auftrag erlaubt Stille und unabhängige Nachbedingung gilt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag definiert absichtlich leeren stdout, unabhängiger Prüfbezug bestätigt die eigentliche verlangte Eigenschaft.

**Warum bleibt oder endet der Zustand?** Abnahme ist für diese Eigenschaft vollständig, nicht bloß für Exitkonvention.

**Zugeordnete Residues:** [OPR034: Unabhängiger aufgabenbezogener Prüfbezug](residues.md#opr034)

**Was bleibt warum nutzbar?** Prüfer kann echten Erfolg trotz Stille belegen ohne Output zu erfinden.

**Zu prüfen:** Behauptete Nachbedingung wurde nicht unabhängig geprüft oder war nicht erfüllt.

**Architekturfolge für diesen Stressor:** Exitcode, Resultatform und fachliche Erfolgseigenschaft separat erfassen. Empty-output darf nur dann korrekt sein wenn der konkrete Auftrag es zulässt.

<a id="s079"></a>
## S079 — Taskprompts enthalten Shell-Metazeichen für Process-Executor

**Ursprung:** operations; A6-Zustände: OP001, OP026, OP027, OP016. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s079.b01"></a>
### S079.B01 — Metazeichen bleiben zugelassene Argumentdaten

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Executor nutzt strukturiertes argv und erlaubtes Executable ohne nachgelagerte Shellauswertung dieser Werte.

**Warum bleibt oder endet der Zustand?** Aufrufübergabe erhält beabsichtigte Bytegrenzen, eigentliche Programmarbeit braucht eigene Abnahme.

**Zugeordnete Residues:** [OPR031: Literal gebundener Prozessauftrag](residues.md#opr031)

**Was bleibt warum nutzbar?** Executor kann Promptdaten mit Metazeichen als Daten übergeben statt Zusatzbefehl.

**Zu prüfen:** Fake-Executor beobachtet Shellinterpretation oder veränderte Argumentgrenzen.

<a id="s079.b02"></a>
### S079.B02 — Shellinterpolation erzeugt nicht beabsichtigten Befehl

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Caller baut Shellstring aus untrusted Prompt, Metazeichen verändern tatsächlich ausgeführten Befehl, Kernrecords bleiben unbetroffen.

**Warum bleibt oder endet der Zustand?** Falsche Ausführung ist geschehen; laufende Nachkommen können weiter schreiben, müssen aber konkret vorhanden sein.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Originaltask kann Absicht belegen, Ausführungs- und Zustimmungsbindung ist verletzt.

**Zu prüfen:** Argumente bleiben literal oder Shellsonderzeichen wirken im konkreten Kontext nicht als Syntax.

**Architekturfolge für diesen Stressor:** Executable und einzelne argv-Werte unveränderlich erfassen und literal ausführen. Das verbietet nicht jede gefährliche Programmbedeutung, verhindert aber ungewollte Shell-Neuinterpretation dieses Pfads.

<a id="s080"></a>
## S080 — Worker stirbt nach fertigem Artefakt vor Result-Commit

**Ursprung:** operations; A6-Zustände: OP034, OP014, OP001, OP012, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s080.b01"></a>
### S080.B01 — Dauerhafte zugeordnete Bytes ohne Resultatcommit

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Worker schrieb geflushte Datei, unabhängige Run-/Versuchzuordnung erhalten, Completion nicht committet.

**Warum bleibt oder endet der Zustand?** Warten endet durch fachliche Prüfung und berechtigte Abschlussentscheidung, nicht durch Dateiexistenz allein.

**Zugeordnete Residues:** [OPR006: Zuordenbare verwaiste Arbeitsbytes](residues.md#opr006), [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002)

**Was bleibt warum nutzbar?** Prüfer kann vorhandene Arbeit nutzen und alten Versuch prüfen ohne blind neu zuzustellen.

**Zu prüfen:** Datei ist nicht dauerhaft zugeordnet oder weiterer Writer startet ohne Budgetentscheidung.

<a id="s080.b02"></a>
### S080.B02 — Geprüfte Originalbytes werden explizit abgenommen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Zuordenbare Orphanbytes und unabhängiger passender Prüfbezug sind verfügbar, autorisierte Completion wird nach Prüfung dauerhaft committet.

**Warum bleibt oder endet der Zustand?** Aufgabe ist erst danach als geprüft abgeschlossen anzusehen.

**Zugeordnete Residues:** [OPR006: Zuordenbare verwaiste Arbeitsbytes](residues.md#opr006), [OPR034: Unabhängiger aufgabenbezogener Prüfbezug](residues.md#opr034), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Resultatnutzer hat nachgewiesene Arbeit und akzeptierten Taskstand, nicht bloß zufällige Datei.

**Zu prüfen:** Prüfung testet falsche Eigenschaft oder Completion wurde nie dauerhaft geschrieben.

<a id="s080.b03"></a>
### S080.B03 — Nicht dauerhafte einzige Arbeitsbytes verloren

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Tod trifft vor tatsächlichem Flush oder Datei wird gelöscht, keine andere vollständige Quelle der einzigartigen Ausgabe vorhanden.

**Warum bleibt oder endet der Zustand?** Berechnung war fertig, benötigte Originalbytes bleiben trotzdem weg.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskabsicht kann neue zulässige Arbeit begründen, exaktes altes Artefakt ist nicht erhalten.

**Zu prüfen:** Lesbare geflushte Originalbytes mit eindeutiger Herkunft werden gefunden.

**Architekturfolge für diesen Stressor:** Orphan-Artefakt, Zustellungewissheit und akzeptierte Completion separat rekonzilieren. Worker-Tod weder als Neuauftrag noch automatisch als fertiges korrektes Ergebnis werten.

<a id="s081"></a>
## S081 — Zahlungsprovider ignoriert Idempotency-Key

**Ursprung:** operations; A6-Zustände: OP022, OP028, OP001, OP023. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s081.b01"></a>
### S081.B01 — Lokaler Intent bleibt ohne sichere Zahlungsantwort

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Provider ignoriert Key, erste Annahme ist lokal unbekannt, Intentjournal vorhanden und kein verbindlicher Lookupbeleg.

**Warum bleibt oder endet der Zustand?** Lokale Abfragen ändern entfernte Wahrheit nicht; weitere Zahlung wird ohne Entscheidung nicht als sicher wiederholbar behandelt.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Operator kann genaue beabsichtigte Zahlung und bisherige Versuche untersuchen, nicht ihren Effektzähler feststellen.

**Zu prüfen:** Host folgert aus vorhandenem Key sichere Wiederholbarkeit trotz Teilnehmervertragsbruch.

<a id="s081.b02"></a>
### S081.B02 — Unabhängige Zahlungsquittung klärt erste Annahme

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Provider bietet trotz ignoriertem Retrykey eine verbindliche eindeutige zugängliche Zahlungsquittung für die erste Operation, kein weiterer ungeklärter Versuch.

**Warum bleibt oder endet der Zustand?** Erste Annahme ist belegt ohne Retry; historischer Vorgang bleibt auch bei späterem Refund wahr.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann diese Zahlung zuordnen und verwenden, nicht generell jeden Retry sicher machen.

**Zu prüfen:** Beleg ist uneindeutig oder es gibt weitere nicht abgedeckte Versuche.

<a id="s081.b03"></a>
### S081.B03 — Wiederholung wird als zweite Zahlung angenommen

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nach möglicher erster Annahme erneuter Zahlungsaufruf, ignorierter Key und zwei tatsächliche Annahmen.

**Warum bleibt oder endet der Zustand?** Finanzielle Mehrbelastung besteht bis spezifischem Refund, historische Doppelzahlung bleibt.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Erhaltene Versuche helfen Diagnose, lokale Keys bewahren keine entfernte Einmaligkeit.

**Zu prüfen:** Teilnehmer belegt tatsächlich nur eine Zahlung oder fachlich waren zwei autorisiert.

**Architekturfolge für diesen Stressor:** Provider-Idempotenz als externen überprüfbaren Vertrag behandeln, nicht durch lokale Keypersistenz ersetzen. Ohne definitive Annahmeevidenz darf Lookup-loses Retry keine Einmaligkeitszusage erhalten.

<a id="s082"></a>
## S082 — Email wurde gesendet aber Versandplugin crasht

**Ursprung:** operations; A6-Zustände: OP028, OP022, OP023. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s082.b01"></a>
### S082.B01 — Email gesendet, Ausgang lokal unbekannt

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Email wurde wie vorgegeben gesendet, Plugin crasht vor durablem Resultat, lokales Intentjournal überlebt.

**Warum bleibt oder endet der Zustand?** Tatsächlicher Versand ist abgeschlossen, lokales Wissen wartet auf echten Beleg oder bewusste Disposition.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator kennt Nachrichtabsicht und Operation, nicht aus sich heraus verbindlichen Versandbeleg.

**Zu prüfen:** Journal fehlt oder verbindlicher eindeutiger Versandbeleg war bereits zugänglich.

<a id="s082.b02"></a>
### S082.B02 — Verbindlicher Versandbeleg überlebt separat

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Zugängliche Teilnehmerquittung belegt genau gesendete Nachricht trotz Plugincrash.

**Warum bleibt oder endet der Zustand?** Diese Versandfrage ist geklärt ohne neue Email; etwaige Fehladressierung bleibt eigener Schaden.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann genau den erfolgten Versand nachweisen und weiteren identischen Versand vermeiden.

**Zu prüfen:** Beleg meint nur Queueaufnahme ohne vertraglichen Versandstatus oder falsche Nachricht.

<a id="s082.b03"></a>
### S082.B03 — Blinder Neuversand schafft zweite irreversible Nachricht

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Crash wird als Nichtversand interpretiert und zweite Nachricht tatsächlich gesendet.

**Warum bleibt oder endet der Zustand?** Beide historischen Zustellungen bleiben, keine lokale Kompensation macht sie ungeschehen.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Absichten und erhaltene Versuche bleiben prüfbar, einmalige Empfängerbelastung ist verloren.

**Zu prüfen:** Nur ein tatsächlicher Versand oder zweite Nachricht war eigenständig gewollt.

**Architekturfolge für diesen Stressor:** Versand als irreversible Annahme mit eigenem Quittungsvertrag führen. Plugincrash ist keine Nichtsendung und lokales Rollback kann Empfängerkenntnis nicht löschen.

<a id="s083"></a>
## S083 — Mensch genehmigt Rechnung A während Payload auf Rechnung B wechselt

**Ursprung:** operations; A6-Zustände: OP017, OP026, OP027, OP028. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s083.b01"></a>
### S083.B01 — Wechsel von A nach B verlangt neue Zustimmung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** A ist als geprüftes Objekt erhalten und verpflichtende effektive Annahmegrenze erkennt B-Abweichung vor Annahme.

**Warum bleibt oder endet der Zustand?** B wartet auf neue inhaltsgebundene Entscheidung, Zustimmung zu A bleibt historisch lesbar.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Entscheider kann exakt alte und neue Rechnung unterscheiden und gezielt prüfen.

**Zu prüfen:** B wird trotz abweichender Bindung unter A-Zustimmung angenommen.

<a id="s083.b02"></a>
### S083.B02 — Rechnung B wurde unter Zustimmung zu A angenommen

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur generischer Freigabestatus statt Inhaltsbindung oder Checkrace, Teilnehmer akzeptiert B, A-Reviewobjekt bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Zustimmungsbruch bleibt historisch, nachträgliche B-Zustimmung autorisiert ihn nicht rückwirkend.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Operator kann betroffene Abweichung anhand erhaltenem A belegen, nicht verlorene Consenttreue wiederherstellen.

**Zu prüfen:** Akzeptierte Rechnung war tatsächlich A oder B entsprach der ausdrücklich erlaubten Variabilität.

<a id="s083.b03"></a>
### S083.B03 — Annahme von B ist zusätzlich verbindlich belegt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Verbindliche Teilnehmerquittung für B liegt vor, unabhängig davon ob Zustimmung wirksam war.

**Warum bleibt oder endet der Zustand?** Ausgangsfrage abgeschlossen, Legitimität bleibt getrennt verletzbar.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann wirklich angenommene Rechnung B als Fakt verfolgen ohne sie als genehmigt umzudeuten.

**Zu prüfen:** Quittung bezieht sich nur auf A-Draft oder vorläufigen Empfang.

**Architekturfolge für diesen Stressor:** Die tatsächlich gezeigte Rechnung als unveränderliches Freigabeobjekt binden und Akzeptanz von B unter A verhindern. Vergleich erst nach Annahme liefert nur Diagnose.

<a id="s084"></a>
## S084 — Freigabe wird nach Annahme beim Teilnehmer widerrufen

**Ursprung:** operations; A6-Zustände: OP028, OP022, OP029. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s084.b01"></a>
### S084.B01 — Irreversible Annahme bleibt trotz Widerruf belegt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Teilnehmer hat endgültig irreversible Wirkung angenommen und verbindliche Quittung ist zugänglich.

**Warum bleibt oder endet der Zustand?** Widerruf sperrt künftige Schritte nur bei wirksamer Grenze, macht frühere Wirkung nicht ungeschehen.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann historischen Effekt nachweisen, nicht zurücknehmen.

**Zu prüfen:** Annahme war nur provisional und verbindlicher Abort verhinderte tatsächlich jeden irreversiblen Effekt.

<a id="s084.b02"></a>
### S084.B02 — Annahme ist real erfolgt aber lokal weiterhin ungewiss

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Widerruf nach Annahme, lokale Bestätigung fehlt, Intentjournal ist erhalten.

**Warum bleibt oder endet der Zustand?** Lokaler Widerruf erzeugt keine entfernte Quittung oder sichere Cancelbestätigung.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Operator sieht bekannte Absicht und muss den alten Ausgang weiter klären.

**Zu prüfen:** Verbindliche Ergebnis-/Abortquittung klärt den Ausgang bereits.

<a id="s084.b03"></a>
### S084.B03 — Kompensierbare Wirkung wartet auf konkrete Korrektur

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Angenommene Wirkung besitzt tatsächlich mögliche Inverse, nötiger Kompensationsstand und Vorbedingungen überleben, neue Korrekturentscheidung fehlt.

**Warum bleibt oder endet der Zustand?** Warten endet durch zulässige überprüfte Mitigation, nicht allein durch Widerruf alter Zustimmung.

**Zugeordnete Residues:** [OPR023: Operationsspezifischer Kompensationsstand](residues.md#opr023)

**Was bleibt warum nutzbar?** Bearbeiter kann sichere konkrete Korrektur beurteilen, noch kein Rollback geschehen.

**Zu prüfen:** Vorzustand fehlt oder Inverse ist tatsächlich irreversibel verlustbehaftet.

**Architekturfolge für diesen Stressor:** Widerruf nach Annahme als Zukunftsgrenze und gegebenenfalls neue Mitigationsentscheidung führen. Keine retroaktive Tilgung einer verbindlich angenommenen Wirkung behaupten.

<a id="s085"></a>
## S085 — Saga kompensiert Änderung die inzwischen ein Mensch weiterbearbeitet hat

**Ursprung:** operations; A6-Zustände: OP029, OP030, OP006, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s085.b01"></a>
### S085.B01 — Versionskonflikt schützt neue menschliche Bearbeitung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompensationsstand enthält erwartete Version, Teilnehmer prüft sie atomar vor Inverse und erkennt menschliche Änderung.

**Warum bleibt oder endet der Zustand?** Inverse wartet auf neue konkrete Entscheidung statt neuere Daten zu überschreiben.

**Zugeordnete Residues:** [OPR023: Operationsspezifischer Kompensationsstand](residues.md#opr023)

**Was bleibt warum nutzbar?** Bearbeiter kann Konflikt und alte Absicht prüfen, erhält keine allgemeine Merge-Lösung.

**Zu prüfen:** Teilnehmer führt Inverse trotz veränderter Version aus oder Check liegt vor einem ungeschützten Race.

<a id="s085.b02"></a>
### S085.B02 — Blinde Inverse löscht einzige neue Bearbeitung

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Teilnehmer überschreibt neuere menschliche Daten, weder deren Version noch vollständige Kopie ist zugänglich.

**Warum bleibt oder endet der Zustand?** Verlorene neue Information bleibt unter diesem Inventar weg; alter Vorzustand könnte trotzdem bekannt sein.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für exakte neue menschliche Arbeit keines; alter Saga-Vorwert rekonstruiert sie gerade nicht.

**Zu prüfen:** Unabhängige vollständige neue Version wird gefunden.

<a id="s085.b03"></a>
### S085.B03 — Neuere Originalversion blieb unabhängig erhalten

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Schädliche Inverse geschah, aber vollständige neue Bearbeitung war vor ihr als zugängliches unveränderliches Artefakt gesichert und interpretierbar.

**Warum bleibt oder endet der Zustand?** Schaden besteht bis erlaubte geprüfte Wiederherstellung, Originalinformation ist weiter verfügbar.

**Zugeordnete Residues:** [OPR005: Erhaltenes originales Artefaktobjekt](residues.md#opr005)

**Was bleibt warum nutzbar?** Berechtigter Bearbeiter kann neuere Bytes lesen und Korrektur vorbereiten, nicht behaupten Teilnehmer sei schon korrekt.

**Zu prüfen:** Gesichertes Objekt enthält nur alten Saga-Vorwert statt der neuen menschlichen Arbeit.

**Architekturfolge für diesen Stressor:** Kompensation an extern durchgesetzte Version und konkrete Merge-/Nachbedingungen binden. Ein gespeicherter alter Vorwert schützt keine inzwischen geschaffene menschliche Arbeit.

<a id="s086"></a>
## S086 — Kompensation schlägt wiederholt fehl

**Ursprung:** operations; A6-Zustände: OP029, OP004. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s086.b01"></a>
### S086.B01 — Fehlgeschlagene Inverse wartet mit bekanntem Stand

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorwärts- und Kompensationsstand dauerhaft vorhanden, weitere automatische Versuche enden am definierten Budget.

**Warum bleibt oder endet der Zustand?** Unveränderte Vorbedingungen erlauben keinen Erfolgsanspruch, kompetente Entscheidung oder Teilnehmeränderung benötigt.

**Zugeordnete Residues:** [OPR023: Operationsspezifischer Kompensationsstand](residues.md#opr023)

**Was bleibt warum nutzbar?** Bearbeiter kann konkrete fehlgeschlagene Korrektur nachvollziehen statt kompletten Rollback behaupten.

**Zu prüfen:** Fehlversuche gehen verloren oder Runner meldet ohne passende Nachbedingung kompensiert.

<a id="s086.b02"></a>
### S086.B02 — Retrylast verstärkt Kompensationsversagen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Inversefehler erzeugen Retries, deren Last senkt Erfolgsrate und erzeugt neue Fehler; begrenztes Hochlastregime hält nach initialem Fehler bei gleichem Grundbedarf.

**Warum bleibt oder endet der Zustand?** Fehler → Retrylast → geringere Korrekturleistung → weitere Fehler. Ein unveränderlich kaputter Teilnehmer allein wäre äußere Blockade.

**Zugeordnete Residues:** [OPR023: Operationsspezifischer Kompensationsstand](residues.md#opr023)

**Was bleibt warum nutzbar?** Nur solange Journal lesbar bleibt kann Operator ursprünglichen Korrekturstand nutzen, Fortschritt ist nicht erhalten.

**Zu prüfen:** Nach Entfernen initialer Störung bei gleicher Last und Policy kehren alle Startzustände zurück oder Retrylast beeinflusst Fehler nicht.

<a id="s086.b03"></a>
### S086.B03 — Unbegrenzte Retryfolge wächst bis Ressourcenende

**Art:** eskalation. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Runner produziert ohne endliches Budget mehr Korrekturaufrufe als System abschließt, keine begrenzte Wiederkehr.

**Warum bleibt oder endet der Zustand?** Wachsende Last endet an Ressourcen statt belegtem Attraktor.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Dauerhafte Erreichbarkeit des Korrekturstands ist ohne Ressourcengrenze nicht nachgewiesen.

**Zu prüfen:** Durchgesetztes Budget hält Journal und lokale Bedienbarkeit erreichbar.

**Architekturfolge für diesen Stressor:** Kompensationsfehler samt benötigtem Vorzustand dauerhaft darstellen, begrenzte weitere Versuche und neue Entscheidung getrennt führen. Wiederholtes Scheitern ist noch kein Attraktor.

<a id="s087"></a>
## S087 — Zwei verschiedene Tasks bestellen fachlich dasselbe Produkt

**Ursprung:** operations; A6-Zustände: OP023, OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s087.b01"></a>
### S087.B01 — Gemeinsame Kaufabsicht begrenzt zwei Tasks auf eine Einheit

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Tasks beziehen sich vor Annahme auf dieselbe vereinbarte Kaufabsicht Menge eins, gemeinsame Annahmegrenze setzt sie wirksam durch.

**Warum bleibt oder endet der Zustand?** Zweiter Auftrag wird derselben fachlichen Annahme zugeordnet statt neu bestellt.

**Zugeordnete Residues:** [OPR024: Fachliche Kaufabsicht](residues.md#opr024)

**Was bleibt warum nutzbar?** Bestellentscheider kann eine Absicht über Taskgrenzen erhalten.

**Zu prüfen:** Zwei getrennte Taskkeys erlauben dennoch zwei Annahmen derselben Menge-eins-Absicht.

<a id="s087.b02"></a>
### S087.B02 — Zwei gewünschte Einheiten sind zwei legitime Bestellungen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Geschäftliche Absicht verlangt ausdrücklich Menge zwei und verbindliche Quittungen belegen diese Annahmen.

**Warum bleibt oder endet der Zustand?** Gleiches Produkt ist kein Duplikationsschaden, wenn beide Einheiten gewollt sind.

**Zugeordnete Residues:** [OPR024: Fachliche Kaufabsicht](residues.md#opr024), [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann tatsächliche Menge gegen autorisierte Absicht prüfen.

**Zu prüfen:** Belege oder Freigabe verlangen tatsächlich nur eine Einheit.

<a id="s087.b03"></a>
### S087.B03 — Zwei unabhängige Tasks kaufen ungewollt doppelt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur per-Task-Keys, tatsächliche Absicht eine Einheit, beide Bestellungen angenommen.

**Warum bleibt oder endet der Zustand?** Historische Mehrbestellung bleibt bis geschäftlicher Korrektur, keine Eigendynamik nötig.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Erhaltene Intentjournale zeigen zwei Vorgänge, erklären aber ohne Fachbezug nicht von selbst Gleichheit der Absicht.

**Zu prüfen:** Fachliche Annahmegrenze verhindert zweite Bestellung oder zwei Einheiten waren gewollt.

**Architekturfolge für diesen Stressor:** Fachliche Kaufabsicht und gewünschte Menge explizit verknüpfen, nicht beliebige gleiche Taskpayloads global deduplizieren. Per-Task-Idempotenz schützt keine absichtsübergreifende Einmaligkeit.

<a id="s088"></a>
## S088 — Provider berechnet beim Retry einen neuen Preis

**Ursprung:** operations; A6-Zustände: OP001, OP027, OP023, OP017, OP022. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s088.b01"></a>
### S088.B01 — Neuer Preis liegt außerhalb erlaubter Terms

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Erhaltenes Reviewobjekt bindet Preis oder Toleranz, wirksame Annahmeprüfung erkennt Abweichung.

**Warum bleibt oder endet der Zustand?** Bis neue Entscheidung kein Kauf zu unerlaubten Terms, ursprünglicher Ausgang bleibt getrennt.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Entscheider kann konkreten Preiswechsel abgrenzen und neu bewerten.

**Zu prüfen:** Teilnehmer nimmt Retry außerhalb der gespeicherten Preisgrenze an.

<a id="s088.b02"></a>
### S088.B02 — Teilnehmer hält dieselbe Bestellung innerhalb erlaubter Terms

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Verbindlicher Operationsbeleg zeigt dieselbe Bestellung und tatsächlicher Preis liegt innerhalb ausdrücklicher ursprünglicher Toleranz.

**Warum bleibt oder endet der Zustand?** Ausgang und Zustimmung passen für diese Operation, bloße Preisänderung ist kein Schaden.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021), [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Operator kann akzeptierte Terms gegen die echte Zustimmung prüfen.

**Zu prüfen:** Retry erzeugt neue Bestellung oder überschreitet erlaubte Terms.

<a id="s088.b03"></a>
### S088.B03 — Alter Ausgang bleibt trotz neuer Preisentscheidung unbekannt

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Erster Auftrag vielleicht angenommen, keine Quittung, lokales Intentjournal erhalten; neuer Preisvorschlag sagt nichts über ersten Ausgang.

**Warum bleibt oder endet der Zustand?** Warten auf verbindliche alte Evidenz, neue Zustimmung wäre nur neue Erlaubnis und kein Nichterfolgsbeweis.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Rekonziliator kann die alte Operation abgrenzen statt Preisrefresh als sicheren Neustart behandeln.

**Zu prüfen:** Neue Preisentscheidung setzt den alten Zustand automatisch auf nicht angenommen.

<a id="s088.b04"></a>
### S088.B04 — Neue Terms ungeprüft angenommen

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Teilnehmer nimmt geänderten Preis außerhalb Zustimmung an, alte Freigabe bleibt lesbar; weitere Doppelbestellung nicht vorausgesetzt.

**Warum bleibt oder endet der Zustand?** Consent-Verstoß bleibt, selbst wenn genau eine Bestellung existiert.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Abweichung von erlaubtem Preis ist untersuchbar, Zustimmungstreue nicht erhalten.

**Zu prüfen:** Tatsächliche Terms lagen innerhalb ausdrücklich erlaubter Toleranz.

**Architekturfolge für diesen Stressor:** Retry behält ursprüngliche Operationsidentität und prüft echte Preisgrenzen. Neue Terms brauchen passende Zustimmung und lösen ungeklärte erste Annahme nicht auf.

<a id="s089"></a>
## S089 — Geplanter Reportversand enthält nach Refresh neue vertrauliche Zeilen

**Ursprung:** operations; A6-Zustände: OP017, OP027, OP028, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s089.b01"></a>
### S089.B01 — Refresh bleibt Draft und verlangt neue Inhaltsprüfung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Neue vertrauliche Zeilen verändern gebundenen Reviewgegenstand, verpflichtende Annahmegrenze stoppt Senden.

**Warum bleibt oder endet der Zustand?** Warten auf korrekte Empfänger-/Inhaltsentscheidung, nicht automatisch alte Zustimmung vererben.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Entscheider kann vorher genehmigten Inhalt abgrenzen und neue Fassung gezielt prüfen.

**Zu prüfen:** Geänderter Rowset wird unter alter Digestfreigabe übertragen.

<a id="s089.b02"></a>
### S089.B02 — Neue Zeilen gehen an unberechtigten Empfänger

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Alte Zustimmung oder Schedule wird pauschal als Versandrecht genutzt, neue vertrauliche Zeilen tatsächlich an nicht berechtigten Empfänger gesendet.

**Warum bleibt oder endet der Zustand?** Erfolgte Kenntnisgabe bleibt auch nach Löschung lokaler Kopie oder Widerruf wahr.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Vertraulichkeit der bereits übermittelten Zeilen keines; kein Unsenden oder perfekte Empfängerlöschung erfunden.

**Zu prüfen:** Keine Übermittlung geschah oder Empfänger war für genau diese neuen Zeilen berechtigt.

<a id="s089.b03"></a>
### S089.B03 — Aktuell geprüfte Fassung wird legitim gesendet

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Neue Fassung ausdrücklich geprüft, Empfänger für alle enthaltenen Zeilen berechtigt, verbindlicher Versandbeleg vorhanden.

**Warum bleibt oder endet der Zustand?** Genau diese Übermittlung ist abgeschlossen, andere spätere Refreshes brauchen neue Bindungsprüfung.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022), [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann zugestimmten Inhalt und tatsächlichen Versand verbinden.

**Zu prüfen:** Versandbeleg meint andere Fassung oder neue Zeilen lagen außerhalb Empfängerrechten.

**Architekturfolge für diesen Stressor:** Reportfreigabe an tatsächlich geprüfte Zeilen Empfänger und Übertragungszweck binden. Refresh eines Drafts ist noch kein Versand, Schedule ist keine Sendeautorisierung.

<a id="s090"></a>
## S090 — Teilnehmer löscht Operationhistorie bevor Factory rekonziliert

**Ursprung:** operations; A6-Zustände: OP022, OP029, OP023, OP001, OP028. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s090.b01"></a>
### S090.B01 — Not-found nach Retentionsende lässt Ausgang offen

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Teilnehmerhistorie gelöscht, nur lokales Intentjournal verfügbar, keine weitere verbindliche Quittung.

**Warum bleibt oder endet der Zustand?** Lokaler Intent liefert keine entfernte Annahmeinformation; weitere gleiche Lookups bleiben ergebnisarm.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Operator kann bekannte Absicht und offene Evidenzlücke untersuchen, keine Effektanzahl rekonstruieren.

**Zu prüfen:** Host behandelt abgelaufenen Lookup als verbindliche Nichtannahme.

<a id="s090.b02"></a>
### S090.B02 — Unabhängig erhaltene verbindliche Quittung reicht für diesen Ausgang

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Authentische eindeutige Quittung wurde tatsächlich außerhalb gelöschter Historie behalten und bleibt rechtmäßig lesbar.

**Warum bleibt oder endet der Zustand?** Konkreter belegter Ausgang ist geklärt, nicht sämtliche unbelegten Teilnehmeroperationen.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann diese Wirkung nachweisen ohne History-Retention oder perfekten Drittbeobachter zu erfinden.

**Zu prüfen:** Quittung existiert nicht mehr oder deckt relevante Operation nicht eindeutig ab.

<a id="s090.b03"></a>
### S090.B03 — Benötigte Kompensationsinformation ebenfalls nicht mehr verfügbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Inverse benötigt einzig beim Teilnehmer gespeicherte gelöschte Version oder Vorinformation, kein anderer vollständiger Bezug erhalten.

**Warum bleibt oder endet der Zustand?** Weder Wiederholung des Lookups noch vorhandene Intent-ID erzeugt diese fehlende Vorbedingung.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für belegbar sichere konkrete Inverse keines; Intentlesen ist eine andere erhaltene Fähigkeit des ersten Zweigs.

**Zu prüfen:** Tatsächlich erhaltener vollständiger Kompensationsstand enthält benötigte Version und Bedeutung.

**Architekturfolge für diesen Stressor:** Lookup-Retentionshorizont und Not-found-Bedeutung pro Teilnehmer führen. Nach Historienlöschung darf fehlender Eintrag nicht zu nie angenommen oder sicher kompensierbar werden.

<a id="s091"></a>
## S091 — Einziger Operator ist drei Tage offline

**Ursprung:** operations; A6-Zustände: OP031, OP017, OP001, OP003, OP005. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s091.b01"></a>
### S091.B01 — Human-gated Aufgabe wartet lesbar auf Owner

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Einziger kompetenter berechtigter Entscheider offline, keine gültige Entscheidung für konkrete Wirkung, Taskkern bleibt gesund.

**Warum bleibt oder endet der Zustand?** Warten bis Rückkehr oder rechtmäßige neue Vertretung; Frist kann währenddessen verfallen.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Operator kann nach Rückkehr Auftrag und offenen Bedarf lesen, nicht verlorene Gelegenheit zurückholen.

**Zu prüfen:** Andere befugte Person kann Entscheidung mit vorhandenem Wissen treffen.

<a id="s091.b02"></a>
### S091.B02 — Unabhängig bereits autorisierte Arbeit geht weiter

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Begrenzte Originalqueue mit weiter gültigen Autorisierungen braucht während Abwesenheit keine neue menschliche Entscheidung.

**Warum bleibt oder endet der Zustand?** Gültige Arbeit kann enden, menschlich abhängige Aufgaben bleiben davon getrennt.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010)

**Was bleibt warum nutzbar?** Lokaler Dispatcher nutzt erhaltene Originalaufträge ohne Abwesenheit als zusätzliche Sendefreigabe auszulegen.

**Zu prüfen:** Aufträge erfordern neue Entscheidung oder Autorisierungen laufen vor Annahme ab.

<a id="s091.b03"></a>
### S091.B03 — Gelegenheit während Wartezeit unwiederbringlich verpasst

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Konkrete zeitgebundene Chance endet vor Owner-Rückkehr und kein legitimer Ersatzweg war vorhanden.

**Warum bleibt oder endet der Zustand?** Rückkehr erneuert vergangene Gelegenheit nicht, kann nur über andere neue Arbeit entscheiden.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskbezug bleibt zur ehrlichen Disposition nutzbar, rechtzeitige Entscheidung ist verloren.

**Zu prüfen:** Chance war verlängerbar oder gültige Entscheidung rechtzeitig vorhanden.

**Architekturfolge für diesen Stressor:** Autorisierte autonome Arbeit von neuen menschlichen Entscheidungen trennen. Drei Tage Abwesenheit sind endliche äußere Kapazitätslücke, keine automatische Fehler-/Alarmrückkopplung.

<a id="s092"></a>
## S092 — Operator bestätigt hundert Dialoge im Alarmsturm reflexhaft

**Ursprung:** operations; A6-Zustände: OP027, OP005, OP032, OP003, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s092.b01"></a>
### S092.B01 — Endlicher Alarmstoß mit tragfähiger Fallprüfung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Alarmzufuhr endet, Aufnahme und Aufmerksamkeit reichen für tatsächlich gespeicherte verständliche Entscheidungsfälle; Klick auf Meldung erteilt keine Wirkungsfreigabe.

**Warum bleibt oder endet der Zustand?** Diese endliche Fallmenge wird geprüft, daraus folgt weder Erledigung unbekannter Vorfälle noch allgemeine Systemgesundheit.

**Zugeordnete Residues:** [OPR032: Benutzbarer Entscheidungsübergabestand](residues.md#opr032), [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Befugter Prüfer kann konkrete Dossiers und Reviewobjekte bearbeiten, nicht bloß hundert Dialoge wegklicken.

**Zu prüfen:** Alarmack autorisiert Wirkung oder ungelöste Fälle werden allein durch Notificationreset als erledigt geführt.

<a id="s092.b02"></a>
### S092.B02 — Reflexfreigabe verursacht einzelne falsche Wirkungen

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Klicks sind bindend, konkrete falsche Wirkung wird angenommen, aber kein Rückweg zu weiteren Alarmen gezeigt; geprüfte Inhalte wurden zumindest festgehalten.

**Warum bleibt oder endet der Zustand?** Einzelner Fehler bleibt ohne notwendige Eigendynamik bis echte Korrektur.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Reviewobjekt belegt was formal vorlag, nicht dass Mensch es verstanden oder richtig entschieden hat.

**Zu prüfen:** Klick war nur Informationsack oder akzeptierte Wirkung war tatsächlich richtig autorisiert.

<a id="s092.b03"></a>
### S092.B03 — Fehlerhafte Freigaben erzeugen neuen Entscheidungsdruck

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Mehr Alarme senken konkrete Prüfqualität, falsche Freigaben verursachen neue Fehler und diese wieder mehr bindende Alarmdialoge; begrenztes Hochlastregime nach anfänglichem Stoß bei unverändertem Grundbedarf.

**Warum bleibt oder endet der Zustand?** Alarmdruck → Reflexfreigaben → neue Fehler → Alarmdruck. Reine Abwesenheit Stummschaltung oder endliche Meldungsmenge genügt nicht.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001), [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Unbetroffene Taskfakten und tatsächlich gespeicherte Reviewobjekte bleiben untersuchbar, Urteilsgüte und vollständiger Vorfallbestand nicht garantiert.

**Zu prüfen:** Nach Stoßende bei gleichem Grundbedarf und Workflow erzeugen genehmigte Fehler keine weiteren Dialoge oder Prüfqualität sinkt nicht.

**Architekturfolge für diesen Stressor:** Alarmquittung strikt von bindender Inhaltsentscheidung trennen und Entscheidungsdossiers an Fälle binden. Endliche Meldungen sind keine vollständige offene Vorfallmenge.

<a id="s093"></a>
## S093 — Vertreter übernimmt ohne Wissen über laufende Außenwirkungen

**Ursprung:** operations; A6-Zustände: OP031, OP022, OP023, OP027, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s093.b01"></a>
### S093.B01 — Vertreter kann erhaltenen Fallstand tatsächlich nutzen

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetenter befugter Vertreter hat zugängliches verständliches Dossier mit belegten Operationen und klar markierten offenen Ausgängen.

**Warum bleibt oder endet der Zustand?** Nach eigener Prüfung kann er erlaubte Entscheidungen treffen; unbekannter Teilnehmerausgang verlangt weiterhin Evidenz.

**Zugeordnete Residues:** [OPR032: Benutzbarer Entscheidungsübergabestand](residues.md#opr032), [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Vertreter kann wissen was bekannt ist und was noch zu klären bleibt ohne Originalowner zu fragen.

**Zu prüfen:** Dossier verschweigt offene Operation oder entscheidende Erklärung ist nur dem Owner bekannt.

<a id="s093.b02"></a>
### S093.B02 — Intent bekannt, entscheidende Annahmeevidenz fehlt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Vertreter versteht Absicht und hat Intentjournal, aber benötigter Teilnehmerausgang ist in keiner verfügbaren Quelle belegt.

**Warum bleibt oder endet der Zustand?** Formale Autorität und längeres Lesen erzeugen fehlende Quittung nicht.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Vertreter kann gezielt den offenen Sachverhalt abgrenzen, nicht sichere Wiederholung folgern.

**Zu prüfen:** Zugänglicher verbindlicher Beleg klärt diesen Ausgang bereits.

<a id="s093.b03"></a>
### S093.B03 — Vertreter errät Nichterfolg und sendet doppelt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Zusätzliche Entscheidung ohne belegten alten Ausgang führt zu neuer Wirkung und beide Annahmen erfolgen.

**Warum bleibt oder endet der Zustand?** Doppelte historische Wirkung bleibt, bessere spätere Übergabe nimmt sie nicht zurück.

**Zugeordnete Residues:** [OPR020: Lokales Außenwirkungs-Intentjournal](residues.md#opr020)

**Was bleibt warum nutzbar?** Erhaltene Intentdaten erlauben Nachuntersuchung, nicht Einmaligkeit oder sachgerechte Entscheidung.

**Zu prüfen:** Keine zweite Annahme oder erneute Wirkung war bewusst eigenständig gewollt.

**Architekturfolge für diesen Stressor:** Übergabe braucht verständliche Evidenz zu konkreten in-flight Operationen und ausdrücklichem Nichtwissen. Kontozugriff allein stellt keine Geschäftskenntnis her.

<a id="s094"></a>
## S094 — Operator verwechselt Produktions- und Fixture-Instanz

**Ursprung:** operations; A6-Zustände: OP026, OP027, OP028, OP024. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s094.b01"></a>
### S094.B01 — Explizite Zielinstanz verhindert Fixture-Produktionsverwechslung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Reviewobjekt nennt vom Menschen tatsächlich beabsichtigte Instanz und verpflichtende Annahmegrenze vergleicht unabhängige echte Instanzidentität.

**Warum bleibt oder endet der Zustand?** Mismatch verlangt neue bewusste Entscheidung statt unbemerkt an anderem Ziel weiterzumachen.

**Zugeordnete Residues:** [OPR022: Inhaltsgebundener Freigabegegenstand](residues.md#opr022)

**Was bleibt warum nutzbar?** Entscheider kann genau beabsichtigtes Ziel gegen echte Verbindung prüfen.

**Zu prüfen:** Gebundenes Fixture-Review wird von Produktion als passende Wirkungserlaubnis akzeptiert.

<a id="s094.b02"></a>
### S094.B02 — Richtig authentifiziert an menschlich falscher Instanz

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Mensch meint Fixture, echte Produktionsmutation wird akzeptiert ohne bindenden Vergleich mit Absicht; Produktionsquittung bleibt zugänglich.

**Warum bleibt oder endet der Zustand?** Falsche Zielwirkung bleibt als Fakt, auch wenn Aufrufer technische Rechte besaß.

**Zugeordnete Residues:** [OPR021: Autoritative Teilnehmerquittung](residues.md#opr021)

**Was bleibt warum nutzbar?** Operator kann tatsächliche Produktionsannahme belegen, menschliche Zieltreue ist verletzt.

**Zu prüfen:** Es erfolgte nur Lesen oder Produktion war tatsächlich beabsichtigt.

<a id="s094.b03"></a>
### S094.B03 — Nur Lesedarstellung wird verwechselt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Kein mutierender Aufruf, Anzeige enthält nicht ausreichend verstandene Instanz-/Frischezuordnung, Kernfakten bleiben richtig.

**Warum bleibt oder endet der Zustand?** Irrtum endet mit verifizierter Kontextklärung, keine bereits erfolgte Außenwirkung.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Aufgabenfakten der tatsächlichen Instanz bleiben lesbar, Interpretation als Fixture ist ungesichert.

**Zu prüfen:** Eine falsche Mutation wurde tatsächlich ausgeführt oder verifizierte Instanz ist bereits klar.

**Architekturfolge für diesen Stressor:** Beabsichtigte Instanz in Reviewobjekt und Akzeptanzkontext binden. Erfolgreiche Authentifizierung zu Produktion sagt nicht dass Mensch Produktion meinte.

<a id="s095"></a>
## S095 — Operator kopiert Backup per Hand statt Restore-Prozess

**Ursprung:** operations; A6-Zustände: OP024, OP011, OP014, OP015. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s095.b01"></a>
### S095.B01 — Manuelle Kopie bleibt lesbarer alter Stand ohne sichere Liveaussage

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Kopie konsistent aber älter, normale Öffnung führt kein Restore-Reconcile aus; spätere reale Versuche können fehlen.

**Warum bleibt oder endet der Zustand?** Wiederholtes Öffnen desselben Präfixes beseitigt weder stale running noch fehlende Fakten.

**Zugeordnete Residues:** [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Bearbeiter kann den enthaltenen Stand auswerten, nicht aktuell running oder nie versucht daraus folgern.

**Zu prüfen:** Unabhängiger Nachlaufbeleg zeigt vollständigen aktuellen Stand oder Kopie ist inkonsistent.

<a id="s095.b02"></a>
### S095.B02 — Nicht sendefähige Inventur schützt vor automatischem Alt-Dispatch

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kopie bleibt technisch ohne Dispatchautorität, zugänglicher Herkunftsstand wird vor autorisierter Übernahme geprüft.

**Warum bleibt oder endet der Zustand?** Alte Statuslabels können gelesen werden ohne neue Wirkungen auszulösen.

**Zugeordnete Residues:** [OPR027: Exklusive Dispatch-Generation](residues.md#opr027), [OPR003: Begrenzter Snapshot-Präfix](residues.md#opr003)

**Was bleibt warum nutzbar?** Wiederhersteller kann Inventur durchführen, fehlenden Nachlauf aber noch nicht rekonstruieren.

**Zu prüfen:** Normales Öffnen oder kopierte Credential erlaubt sofort effektfähigen Dispatch.

<a id="s095.b03"></a>
### S095.B03 — Explizites Reconcile hält enthaltene Versuche und Leases

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** C2 wird auf enthaltene running/attempted Rows und leasehaltende Sessions angewandt, diese Datensätze überleben.

**Warum bleibt oder endet der Zustand?** Zustell- und Belegungssperren bleiben getrennt bis jeweilige Evidenz/Entscheidung vorliegt.

**Zugeordnete Residues:** [OPR002: Zustellversuch mit verbrauchter Autorisierung](residues.md#opr002), [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Operator kann gespeicherte Versuche und Workspacebelegung nutzen; nicht enthaltene spätere Versuche bleiben außerhalb.

**Zu prüfen:** Reconcile entfernt Leases oder erklärt queued-ohne-Row global nie zugestellt.

**Architekturfolge für diesen Stressor:** Manuelle Kopie als möglicherweise alten Herkunftsstand erkennen und zunächst von Dispatch trennen. Explizites C2-Reconcile verändert nur vorhandene Rows und füllt keinen fehlenden Nachlauf.

<a id="s096"></a>
## S096 — Mensch beendet pane manuell und schreibt später im selben Workspace

**Ursprung:** operations; A6-Zustände: OP015, OP016, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s096.b01"></a>
### S096.B01 — Pane weg, verwaltete Wiedervergabe wartet

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Belegung wird bei fehlendem Prozess-/Schreibnachweis erhalten, menschlicher späterer Zugriff ist noch nicht koordiniert.

**Warum bleibt oder endet der Zustand?** Bis alte Schreiber und neue menschliche Änderungen geklärt sind kein kontrollierter Ersatzstart.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Startentscheider behält alte Belegung als Sperrgrund, nicht Macht über menschliche Editoren.

**Zu prüfen:** Paneverlust allein gibt Workspace frei oder alternative Reconnectfunktion umgeht Sperre.

<a id="s096.b02"></a>
### S096.B02 — Mensch und Ersatzwriter schreiben überlappend

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Mensch editiert nach Paneende, verwalteter Ersatz wurde ohne Übergabe ebenfalls gestartet.

**Warum bleibt oder endet der Zustand?** Konflikt dauert mit beiden Aktivitäten, Ende einer Pane ist keine beiderseitige Übergabe.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Unbetroffener Taskkern bleibt nutzbar, exklusive Änderungsattribution fehlt.

**Zu prüfen:** Alle Schreiber waren abgestimmt und nicht überlappend aktiv.

<a id="s096.b03"></a>
### S096.B03 — Geprüfte Übergabe erhält konkreten Arbeitsstand

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Alte Prozessschreibrechte geklärt, menschlicher Stand tatsächlich unveränderlich übernommen und fachlich relevante Änderungen vor Neuaufnahme geprüft.

**Warum bleibt oder endet der Zustand?** Kontrollierte Weiterarbeit nutzt benannten Stand statt unbekannte alte Pfadannahmen.

**Zugeordnete Residues:** [OPR005: Erhaltenes originales Artefaktobjekt](residues.md#opr005)

**Was bleibt warum nutzbar?** Nachfolger kann den übergebenen Originalstand lesen, nicht beliebige spätere menschliche Edits mitumfassen.

**Zu prüfen:** Übernahme erfolgte nicht oder menschliche Änderungen passieren weiter unkoordiniert nach ihr.

**Architekturfolge für diesen Stressor:** Pane-Schließung, Prozessbaum und menschliche Workspaceübergabe getrennt führen. Agenten dürfen Taskdateien schreiben, Factory-Lease kann legitime menschliche Änderungen nicht physisch verhindern.

<a id="s097"></a>
## S097 — Nur zuständiger Owner versteht eine Blockerbeschreibung

**Ursprung:** operations; A6-Zustände: OP031, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s097.b01"></a>
### S097.B01 — Nur Owner versteht dauerhaften Blocker

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Owner fehlt, vorhandener Text enthält keine für anderen kompetenten Bearbeiter ausreichende Bedeutung und keine weitere erklärende Quelle.

**Warum bleibt oder endet der Zustand?** Wiederholte Eskalation desselben Texts erzeugt kein fehlendes Wissen.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Task-ID und Status bleiben nachlesbar, für selbständige Blockerentscheidung keines.

**Zu prüfen:** Befugter Vertreter löst ihn korrekt aus vorhandenen Fakten ohne Ownerkontakt.

<a id="s097.b02"></a>
### S097.B02 — Tatsächlich verständlicher Fallstand ermöglicht Entscheidung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Verfügbare Erklärung nennt fehlende Voraussetzung Belege Zuständigkeit und erlaubte Optionen verständlich für kompetenten berechtigten Ersatz.

**Warum bleibt oder endet der Zustand?** Nach Prüfung kann Entscheidung erfolgen, sofern kein weiterhin fehlender externer Fakt benötigt wird.

**Zugeordnete Residues:** [OPR032: Benutzbarer Entscheidungsübergabestand](residues.md#opr032)

**Was bleibt warum nutzbar?** Vertreter kann Fall bearbeiten statt nur blocked erneut zu quittieren.

**Zu prüfen:** Übung zeigt dass entscheidende Bedeutung weiterhin exklusives Ownerwissen ist.

**Architekturfolge für diesen Stressor:** Blocker als prüfbaren Entscheidungsfall strukturieren und Verständnis durch befugten Ersatz testen. Persistente Zeichenfolge allein ist kein nutzbares Übergaberesidue.

<a id="s098"></a>
## S098 — Operator ist bei Abnahme seines eigenen fehlerhaften Ergebnisses befangen

**Ursprung:** operations; A6-Zustände: OP005, OP026, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s098.b01"></a>
### S098.B01 — Selbstabnahme bewahrt falsches Ergebnis

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Ergebnis ist tatsächlich fehlerhaft, befangene Abnahme akzeptiert es, unabhängiger passender Prüfbezug fehlt.

**Warum bleibt oder endet der Zustand?** Ein falscher angenommener Stand bleibt bis neue sachliche Evidenz; ohne Wiederverwendungskante kein Attraktor.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Taskrecord dokumentiert Annahme, beweist weder Unbefangenheit noch sachliche Wahrheit.

**Zu prüfen:** Ergebnis war richtig oder Fehler wurde vor Annahme durch verpflichtende Prüfung erkannt.

<a id="s098.b02"></a>
### S098.B02 — Unabhängiger aufgabenbezogener Vergleich fängt Fehler

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Unbetroffener konkreter Prüfbezug zur Fehlerklasse existiert und ist verpflichtend vor fachlicher Abnahme.

**Warum bleibt oder endet der Zustand?** Abnahme wartet auf korrigiertes geprüftes Ergebnis, nicht lediglich anderen zustimmenden Namen.

**Zugeordnete Residues:** [OPR034: Unabhängiger aufgabenbezogener Prüfbezug](residues.md#opr034)

**Was bleibt warum nutzbar?** Prüfer kann Fehler unabhängig vom Autorinteresse feststellen; andere ungemessene Eigenschaften bleiben offen.

**Zu prüfen:** Prüfbezug teilt denselben Fehler oder misst nur Form statt der beschädigten Eigenschaft.

**Architekturfolge für diesen Stressor:** Fachliche Abnahme von Autorenschaft und formaler Completion trennen. Unabhängige Prüfung muss konkret betroffene Eigenschaft messen, zweite Zustimmung allein ist kein Orakel.

<a id="s099"></a>
## S099 — Mensch fordert Sofortstart trotz unklarer alter Lease

**Ursprung:** operations; A6-Zustände: OP026, OP015, OP016, OP001. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s099.b01"></a>
### S099.B01 — Sofortstart wird bei unklarer alter Lease zurückgehalten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alter stabiler Workspace im selben Store belegt, neuer Start benutzt C10 und keine Umgehung.

**Warum bleibt oder endet der Zustand?** Warten endet durch passende alte Schreibfähigkeitsklärung oder zulässige Disposition, nicht durch Dringlichkeitswort.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015)

**Was bleibt warum nutzbar?** Startentscheider kann erhaltene Belegung weiterhin zur kontrollierten Ausschließung nutzen.

**Zu prüfen:** begin_start akzeptiert zweite Reservation trotz stabiler alter Lease.

<a id="s099.b02"></a>
### S099.B02 — Erzwungener Bypass schafft zwei Schreiber

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Mensch oder anderer Recovery-Pfad gibt frei/startet direkt, alter Prozess hat weiterhin Schreibfähigkeit und Ersatz startet wirklich.

**Warum bleibt oder endet der Zustand?** Überlappung besteht solange beide schreiben, Anfrage nach Sofortstart allein genügt nicht.

**Zugeordnete Residues:** [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Unbetroffene Taskdaten bleiben, exklusive Workspacekontrolle nicht.

**Zu prüfen:** Kein Ersatz wurde gestartet oder alte Schreibfähigkeit war zuvor positiv erloschen.

<a id="s099.b03"></a>
### S099.B03 — Nach tatsächlicher Klärung kontrollierte Wiederaufnahme

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Alte Schreibfähigkeiten sind nachweislich erloschen oder identisch korreliert, dokumentierter Taskstand und Belegung werden konsistent aktualisiert.

**Warum bleibt oder endet der Zustand?** Nur danach kann kontrollierte Zuordnung weitergehen; alte Teilnehmerwirkungen bleiben eigene Frage.

**Zugeordnete Residues:** [OPR015: Dauerhafte Workspace-Belegung](residues.md#opr015), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Aufnahmeentscheider nutzt geklärte Belegung und vorhandene Taskfakten statt Dringlichkeit als Beweis.

**Zu prüfen:** Klärung besteht nur aus Paneabwesenheit oder verdeckt weiter schreibende Nachkommen.

**Architekturfolge für diesen Stressor:** Dringlichkeit darf Belegungsevidenz nicht ersetzen. Normaler Leaseguard und manuelle/Recovery-Bypässe getrennt auditiert halten statt pauschal sichere Starts behaupten.

<a id="s100"></a>
## S100 — Operator verliert alle Zugänge und Recoveryanweisungen liegen nur auf dem Gerät

**Ursprung:** operations; A6-Zustände: OP013, OP031, OP001, OP006. [Originalverläufe](../../../../reviews/operations/trajectories.csv)

<a id="s100.b01"></a>
### S100.B01 — Zugänge und Anleitung liegen hinter derselben Sperre

**Art:** halt. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Alle aktuell nutzbaren Operatorzugänge fehlen, Anleitung nur auf unerreichbarem Gerät, weitere Wege sind nicht belegt.

**Warum bleibt oder endet der Zustand?** Warten bleibt bis echter unabhängiger Zugang oder Bestandsklärung, intakte Bits allein lösen Sperre nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für selbständigen Besitzerwiederzugang kein nutzbarer Gegenstand belegt; dauerhafte Vernichtung wird daraus noch nicht gefolgert.

**Zu prüfen:** Eine legitime außerhalb dieser Sperre erreichbare Anleitung samt ausreichendem Zugangspfad wird gefunden.

<a id="s100.b02"></a>
### S100.B02 — Wirklich unabhängiger Recoveryweg besteht

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Zusätzlicher bereits eingerichteter legitimer Wiederzugang besitzt zugängliche Anleitung und erforderliche Identitäts-/Schlüsselpfade außerhalb verlorenen Geräts.

**Warum bleibt oder endet der Zustand?** Nach erfolgreicher autorisierter Wiedergewinnung kann Besitzer Zustand prüfen, nicht automatisch alte Effekte wiederholen.

**Zugeordnete Residues:** [OPR033: Unabhängiger legitimer Wiederzugang](residues.md#opr033)

**Was bleibt warum nutzbar?** Besitzer kann unter genau diesem Ausfall Zugang zurückerlangen; kein geheimer Inhalt im Analysebericht nötig.

**Zu prüfen:** Recovery verlangt als ersten Schritt einen der verlorenen Zugänge oder einzige gesperrte Anleitung.

<a id="s100.b03"></a>
### S100.B03 — Lokale autorisierte Arbeit lebt ohne Besitzerzugriff

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Gerät und Runtime laufen gesund, vorhandene begrenzte autorisierte Originalaufträge brauchen keinen neuen menschlichen Zugang.

**Warum bleibt oder endet der Zustand?** Arbeit kann weitergehen solange Autorisierung und Ressourcen genügen, Besitzer bleibt blind.

**Zugeordnete Residues:** [OPR010: Begrenzter ursprünglicher Arbeitsvorrat](residues.md#opr010), [OPR001: Lesbarer Taskkern](residues.md#opr001)

**Was bleibt warum nutzbar?** Lokaler Worker kann erhaltene Aufträge nutzen, nicht der ausgesperrte Besitzer aktuell inspizieren.

**Zu prüfen:** Laufende Arbeit benötigt neuen Ownerentscheid oder Gerät ist ebenfalls außer Betrieb.

<a id="s100.b04"></a>
### S100.B04 — Einzige erforderliche Recoveryinformation endgültig zerstört

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Alle nötigen Schlüssel-/Identitätsinformationen und zulässigen Rekonstruktionsquellen sind nach geschlossenem Inventar unwiederbringlich verloren.

**Warum bleibt oder endet der Zustand?** Keine erreichbare Anleitung ersetzt fehlende einzigartige Autoritäts-/Entschlüsselungsinformation.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die davon abhängige Besitzerwiederherstellung keines; keine imaginäre externe Kopie oder perfekte Kontorettung.

**Zu prüfen:** Tatsächlich ausreichender legitimer Recoverypfad oder vollständige Schlüsselquelle wird nachgewiesen.

**Architekturfolge für diesen Stressor:** Recoveryabhängigkeiten einschließlich rechtmäßig erreichbarer Anleitung Schlüssel und Autorität außerhalb derselben verlorenen Grenze prüfen. Lokaler Safe-Mode kann keine verlorenen Kontorechte oder physische Erreichbarkeit erzeugen.

<a id="s101"></a>
## S101 — Modellanbieter schließt über Nacht

**Ursprung:** governance; A6-Zustände: GV04, GV01, GV07. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s101.b01"></a>
### S101.B01 — Inference steht, alte Arbeit bleibt lesbar

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Provider ist notwendig und geschlossen; lokaler Store und erlaubter Leser bleiben erreichbar.

**Warum bleibt oder endet der Zustand?** Dauert bis ein zulässiger Ersatz verfügbar ist; kein interner Kreislauf.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Berechtigte Prüfer können gespeicherte Aufgaben und Versuche lesen; neue Inference bleibt unmöglich.

**Zu prüfen:** Ohne Provider lassen sich die bezeichneten alten Aufgaben nicht lesen.

<a id="s101.b02"></a>
### S101.B02 — Begrenzte Aufgabe mit Ersatz abgeschlossen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein verfügbares erlaubtes Ersatzmodell erzeugt ein unabhängig abgenommenes Ergebnis; Bytes bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Endliche Abnahme beendet diese Aufgabe, nicht sämtliche Providerabhängigkeiten.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002)

**Was bleibt warum nutzbar?** Empfänger nutzt das konkret abgenommene Ergebnis; der Entwurf behauptet keine identischen Modelle.

**Zu prüfen:** Abnahme scheitert am fachlichen Zweck oder Ergebnis benötigt den geschlossenen Dienst.

**Architekturfolge für diesen Stressor:** Providerabhängigkeit vom lokalen Aktenlesen trennen; Ersatzmodell erst nach auftragsbezogener Abnahme verwenden, nicht als historische Identität des geschlossenen Providers ausgeben.

<a id="s102"></a>
## S102 — Modellalias wird still auf andere Gewichte umgestellt

**Ursprung:** governance; A6-Zustände: GV07, GV05, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s102.b01"></a>
### S102.B01 — Aliasvergleich ohne Gewichtszuordnung

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Anbieter liefert für den stillen Wechsel keine vertrauenswürdige historische Gewichtsidentität; Rohwerte bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Spätere Aliasabfragen ersetzen keine fehlende frühere Identität.

**Zugeordnete Residues:** [GVR004: Rohmessungen mit begrenztem Vergleich](residues.md#gvr004)

**Was bleibt warum nutzbar?** Analyst kann bekannte Ergebnisgrößen auswerten, aber keine Gewichtsversion zuschreiben.

**Zu prüfen:** Ein zeitgenössischer unabhängiger Aufrufbeleg klärt die genaue Version.

<a id="s102.b02"></a>
### S102.B02 — Fehler wird nach Wechsel zur Referenz

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Neue falsche Ausgabe wird wiederholt als Autorität übernommen; widersprechende Daten beeinflussen Entscheidungen nicht.

**Warum bleibt oder endet der Zustand?** Falsche Ausgabe → spätere Zitate → wachsendes Vertrauen → weniger Gegenprüfung → erneute Fehlerannahme.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Erhaltene Akte lässt Aussagen und Wiederverwendung untersuchen; sie liefert keine Wahrheit oder verlorene Gewichtsdaten.

**Zu prüfen:** Fehler wird nicht weiterverwendet oder ein Gegenbefund verändert trotz hoher Zustimmung die Entscheidung.

**Architekturfolge für diesen Stressor:** Pro Aufruf belegte Modellrevision von Alias und Eingaberevision trennen; unbekannte Gewichte nicht aus späteren Wiederholungen rekonstruieren.

<a id="s103"></a>
## S103 — Modell folgt Prompt Injection in einer Quellseite

**Ursprung:** governance; A6-Zustände: GV09, GV08, GV05, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s103.b01"></a>
### S103.B01 — Injection befolgt, Toolwirkung verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Modell folgt der Seite, aber alle verlangten nicht erlaubten Effekte treffen eine unabhängige durchgesetzte Sperre.

**Warum bleibt oder endet der Zustand?** Sperre besteht bis konkrete gültige Neubewertung; Text allein öffnet sie nicht.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer behalten den Effektauftrag und können ihn ablehnen; keine privilegierte Wirkung dieser Anforderung.

**Zu prüfen:** Ein Toolnebenpfad nimmt die injizierte Wirkung trotzdem an.

<a id="s103.b02"></a>
### S103.B02 — Injection hat geschützte Daten offengelegt

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Toolbefugnis erlaubt Übertragung und unberechtigter Empfänger hat den betreffenden Inhalt gelesen.

**Warum bleibt oder endet der Zustand?** Das Wissen beim Empfänger bleibt historisch erfolgt; späterer Stop kann es nicht zurückrufen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Nie-Offengelegtheit dieser Daten bleibt keine Restfähigkeit; andere Daten sind damit nicht automatisch verloren.

**Zu prüfen:** Empfänger konnte den Inhalt nachweislich nicht lesen.

<a id="s103.b03"></a>
### S103.B03 — Falsche Quellbehauptung verfestigt sich

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Injizierte Behauptung wird in weitere Kontexte kopiert und als unabhängige Bestätigung behandelt.

**Warum bleibt oder endet der Zustand?** Kopie → Vertrauen → erneute Übernahme → mehr scheinbare Quellen; ursprüngliche Website kann verschwinden.

**Zugeordnete Residues:** [GVR003: Gebundenes Eingabepaket](residues.md#gvr003)

**Was bleibt warum nutzbar?** Ein zuvor gebundenes Eingabepaket erlaubt dem Prüfer Herkunftsanalyse, sofern es außerhalb der Manipulation erhalten ist; es korrigiert den Glauben nicht automatisch.

**Zu prüfen:** Keine Wiederverwendung oder entscheidungswirksame Prüfung unterbricht die Kette.

**Architekturfolge für diesen Stressor:** Quellinhalt darf keine Toolbefugnis liefern. Vollständige Effektvermittlung und OS-Datengrenze getrennt prüfen; Reaktion auf Injection ist keine Rücknahme bereits gelesener Geheimnisse.

<a id="s104"></a>
## S104 — Zwei Prüfer nutzen denselben Modellfehler und bestätigen falsches Ergebnis

**Ursprung:** governance; A6-Zustände: GV05, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s104.b01"></a>
### S104.B01 — Einmalige falsche Doppelbestätigung

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Beide Prüfer irren; danach wird das Urteil nicht als Quelle wiederverwendet und kein Vertrauenskreislauf entsteht.

**Warum bleibt oder endet der Zustand?** Endlicher Bewertungsfehler; späterer Ausgang ist ohne weitere Dynamik offen.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Erhaltene Bewertungen erlauben Nachprüfung des Vorgangs, nicht des behaupteten Wahrheitsgehalts.

**Zu prüfen:** Die Bestätigung wird später zitiert und steigert die Wahrscheinlichkeit weiterer Annahmen.

<a id="s104.b02"></a>
### S104.B02 — Doppelurteil zirkuliert als Beweis

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Falsches Urteil erhält Autorität und spätere Prüfer übernehmen es statt Sachprüfung.

**Warum bleibt oder endet der Zustand?** Übereinstimmung → Vertrauen → Wiederverwendung → weitere Übereinstimmung → Ausschluss von Gegenprüfung.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Die unveränderte Bewertungsakte bleibt ein untersuchbarer Gegenstand; berechtigte Prüfer können den Zitatkreislauf sehen, sofern lesbar.

**Zu prüfen:** Adverse unabhängige Sachbefunde werden akzeptiert und beenden die Übernahme.

<a id="s104.b03"></a>
### S104.B03 — Sachprüfung stoppt falsche Abnahme

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Eine für diesen Fehler unabhängige Prüfung existiert und ihr negativer Befund darf die Abnahme blockieren.

**Warum bleibt oder endet der Zustand?** Halt endet mit korrigiertem Ergebnis oder legitimem Abbruch, nicht durch weitere gleiche Reviewer.

**Zugeordnete Residues:** [GVR006: Unabhängiger fachlicher Gegenbefund](residues.md#gvr006), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Befund und gesperrter Auftrag bleiben für zuständige Prüfer nutzbar; kein allwissender dritter Prüfer wird vorausgesetzt.

**Zu prüfen:** Der gleiche Modellfehler täuscht auch den Sachtest oder negatives Ergebnis wird ignoriert.

**Architekturfolge für diesen Stressor:** BR01 übernehmen: zwei falsche Bestätigungen sind zunächst ein Einzelfehler. Unabhängige Abnahme muss die konkrete Fehlerfamilie treffen und Entscheidungen tatsächlich verändern können.

<a id="s105"></a>
## S105 — Modell halluziniert nicht existierende Ergebnisdateien

**Ursprung:** governance; A6-Zustände: GV03, GV05, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s105.b01"></a>
### S105.B01 — Dateipfad behauptet, Original fehlt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Gespeicherter Abschluss nennt eine nicht existierende Datei; sonstiger Aufgabenbericht ist lesbar.

**Warum bleibt oder endet der Zustand?** Erneutes Lesen desselben Pfadstrings erzeugt keinen Inhalt; eine neue Datei wäre neue Arbeit.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Prüfer kann die falsche Ergebnisbehauptung belegen; für die behaupteten Originalbytes gibt es keine Restfähigkeit.

**Zu prüfen:** Zeitgenössische Originaldatei mit passender Identität wird tatsächlich gefunden.

<a id="s105.b02"></a>
### S105.B02 — Vorhandenes Ergebnis separat abgenommen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Neben halluziniertem Namen existieren echte relevante Ergebnisbytes und deren konkrete Abnahme.

**Warum bleibt oder endet der Zustand?** Nur der belegte Ergebnisumfang wird abgeschlossen; falsche Artefaktbehauptung bleibt sichtbar.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002), [GVR037: Eindeutig adressierbarer Artefaktbestand](residues.md#gvr037)

**Was bleibt warum nutzbar?** Empfänger nutzt identifizierte abgenommene Bytes, nicht den ungeprüften Pfad aus complete_with_result.

**Zu prüfen:** Objektmanifest zeigt keine erreichbaren Bytes oder fachliche Abnahme hängt an der erfundenen Datei.

**Architekturfolge für diesen Stressor:** Bibliotheksabschluss nicht mit Dateiabnahme gleichsetzen: Pfad, tatsächliche Bytes und fachlich akzeptierter Inhalt benötigen getrennte Nachweise.

<a id="s106"></a>
## S106 — Lokales Modell belegt gesamten RAM und verdrängt Daemon

**Ursprung:** governance; A6-Zustände: GV04, GV13, GV10. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s106.b01"></a>
### S106.B01 — Modell verdrängt Daemon, Akte überlebt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Daemon fällt durch RAMdruck aus, bereits bestätigte Daten bleiben intakt und später lokal lesbar.

**Warum bleibt oder endet der Zustand?** Modellbelegung hält Ausführung an; Neustart ohne Änderung kann dieselbe Überlast erneut erzeugen.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Operator liest nach verfügbarem Speicher alte Versuche; dies belegt weder weiterlaufende Arbeit noch deren Ende.

**Zu prüfen:** RAMausfall hinterlässt unlesbaren Store oder der benötigte Leser startet auch nach Freigabe nicht.

<a id="s106.b02"></a>
### S106.B02 — Unklare Zustellung bleibt gesperrt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Wiederanlauf kennt einen Versuch und eine funktionierende Effektsperre; Prozessbeobachtung fehlt.

**Warum bleibt oder endet der Zustand?** Outcome-Unklarheit und Leasebelegung haben getrennte Ausgänge; RAMfreigabe allein beendet sie nicht.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer kann die offene Effekthülle bearbeiten; dies ist eine entworfene Vermittlungsgrenze, keine produktweite Garantie aus restore::reconcile.

**Zu prüfen:** Ein anderer Recoverypfad nimmt ohne diesen Abgleich neue Wirkung an.

**Architekturfolge für diesen Stressor:** Modell-RAM und Betriebsreserve begrenzen; Recoveryanzeige muss Datenträgerhaltbarkeit, Zustellunsicherheit und Leasebelegung unabhängig führen.

<a id="s107"></a>
## S107 — Modell wechselt mitten im Run ohne Telemetriehinweis

**Ursprung:** governance; A6-Zustände: GV07, GV05. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s107.b01"></a>
### S107.B01 — Ergebnis bleibt, Erzeugerabschnitt unbekannt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Modellwechsel wurde nicht aufgezeichnet; tatsächliche Ausgabe ist erhalten und zweckspezifisch unabhängig abgenommen.

**Warum bleibt oder endet der Zustand?** Fehlende Aufrufidentität bleibt offen, auch wenn das Ergebnis korrekt verwendbar ist.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002), [GVR004: Rohmessungen mit begrenztem Vergleich](residues.md#gvr004)

**Was bleibt warum nutzbar?** Empfänger nutzt die abgenommenen Bytes; Analyst vergleicht nur belegte Größen, nicht erfundene Modellanteile.

**Zu prüfen:** Abnahme ist modellzirkulär oder ein unabhängiger Aufrufbeleg rekonstruiert doch alle Abschnitte.

<a id="s107.b02"></a>
### S107.B02 — Neue Fehler werden ohne Wechselhinweis übernommen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Verdeckter Wechsel erzeugt Fehler, die in weitere Aufträge als gültige Quellen eingehen.

**Warum bleibt oder endet der Zustand?** Übernahme → Bestätigung aus kopierten Quellen → Vertrauen → weitere Übernahme.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Lesbare Auftragsakten zeigen den Behauptungsfluss, nicht die fehlende Modellidentität.

**Zu prüfen:** Die Fehler werden nicht wiederverwendet oder wirksame Sachprüfung korrigiert sie.

**Architekturfolge für diesen Stressor:** Per-Aufruf-Zuordnung als eigene Evidenzlücke markieren. Run-Metadatum oder Eingabehash dürfen einen unbeobachteten Modellwechsel nicht verdecken.

<a id="s108"></a>
## S108 — Modell produziert unendlich viele korrekte Zwischenschritte

**Ursprung:** governance; A6-Zustände: GV12, GV21, GV04, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s108.b01"></a>
### S108.B01 — Korrekte Teilarbeit ohne Ende

**Art:** eskalation. **Residue-Status:** teilweise.

**Voraussetzungen:** Jeder akzeptierte Schritt erzeugt einen weiteren; kein wohlfundiertes Ende, aber haltbare kleine Zwischenstände sind vorhanden.

**Warum bleibt oder endet der Zustand?** Weitere Schritte erhöhen Kosten; endliche Energie beendet die idealisierte Nichttermination.

**Zugeordnete Residues:** [GVR028: Wiederaufnehmbarer Arbeitsstand](residues.md#gvr028)

**Was bleibt warum nutzbar?** Worker kann belegte Teilarbeit später fortsetzen oder prüfen; ein terminaler Liefergegenstand ist nicht erhalten.

**Zu prüfen:** Eine endliche Restmaßzahl sinkt bei jedem Schritt und erreicht null.

<a id="s108.b02"></a>
### S108.B02 — Endlicher Zweck wird erreicht

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein präziser endlicher Teilauftrag wird legitim vereinbart und unabhängig abgenommen.

**Warum bleibt oder endet der Zustand?** Erfüllte Abnahme beendet nur diesen Teilauftrag; vorherige offene Ziele bleiben getrennt.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002)

**Was bleibt warum nutzbar?** Empfänger verwendet das konkret fertige Ergebnis, nicht den bloßen Zähler korrekter Zwischenschritte.

**Zu prüfen:** Die Abnahme enthält weiterhin unbegrenzt neue Pflichten.

**Architekturfolge für diesen Stressor:** Terminales Abnahmekriterium und Checkpoint von lokaler Schrittgüte trennen; endlicher Ressourcenstop ist kein Beweis endloser Ausführung.

<a id="s109"></a>
## S109 — Kontextfenster schrumpft nach Harnessupgrade

**Ursprung:** governance; A6-Zustände: GV02, GV07, GV05, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s109.b01"></a>
### S109.B01 — Kleineres Fenster verweigert alten Kontext

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Harness erkennt Überschreitung vor Arbeit; gebundenes Eingabepaket und vollständige Sperre bestehen.

**Warum bleibt oder endet der Zustand?** Halt bis ein gültig gekürzter Auftrag freigegeben oder der Run abgebrochen wird.

**Zugeordnete Residues:** [GVR003: Gebundenes Eingabepaket](residues.md#gvr003), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer kann die betroffenen Eingaben sehen und genau diese Ausführung zurückhalten.

**Zu prüfen:** Harness arbeitet trotzdem mit still abgeschnittenen Pflichtanweisungen.

<a id="s109.b02"></a>
### S109.B02 — Unbeobachtete Trunkierung

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Harness schneidet still ab und zeichnet verbrauchte Teile nicht auf; ursprüngliche Compilerbytes bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Späteres erneutes Kompilieren rekonstruiert den früheren tatsächlichen Verbrauch nicht.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Auftragsakte bleibt lesbar; für den tatsächlichen vollständigen Verbrauch fehlt Evidenz, weshalb GVR003 nicht als erfüllt zählt.

**Zu prüfen:** Eine unabhängige Empfangsspur enthält die vollständigen damals verbrauchten Bytes.

**Architekturfolge für diesen Stressor:** Kompilierte Bytes und vom Harness verbrauchten Kontext getrennt belegen; vor Kürzung Regelvollständigkeit prüfen statt bloßer Bytezählung.

<a id="s110"></a>
## S110 — Benchmarkfixture wird vom Modell erkannt und speziell ausgetrickst

**Ursprung:** governance; A6-Zustände: GV06, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s110.b01"></a>
### S110.B01 — Bekannter Test ersetzt unabhängige Prüfung

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Erkannte Fixture wird ausgetrickst und hoher Score bewirkt Abschaffung anderer Prüfungen.

**Warum bleibt oder endet der Zustand?** Gaming → Score → Vertrauen → weniger Fremdtests → unentdecktes Gaming.

**Zugeordnete Residues:** [GVR004: Rohmessungen mit begrenztem Vergleich](residues.md#gvr004)

**Was bleibt warum nutzbar?** Analyst kann den gespeicherten engen Testwert lesen; Generalisierung und reale Kompetenz bleiben unbewiesen.

**Zu prüfen:** Zurückgehaltene Sachtests bleiben entscheidungswirksam trotz hoher Scores.

<a id="s110.b02"></a>
### S110.B02 — Optimierung bleibt ehrlich fixturebegrenzt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Behauptung betrifft nur die bekannte Fixture und deren tatsächliches Ergebnis, nicht allgemeine Fähigkeit.

**Warum bleibt oder endet der Zustand?** Der enge Testlauf endet; daraus folgt kein anhaltender Fehlbewertungskreislauf.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Prüfer nutzt den explizit begrenzten Testbeleg und seine Ausschlüsse.

**Zu prüfen:** Der enge Score wird doch zur Freigabe ungeprüfter Aufgabenklassen verwendet.

**Architekturfolge für diesen Stressor:** Bekannte Fixtureleistung als engen Anspruch führen; Unabhängigkeit eines Tests nicht allein aus ausführbarem statt modellbasiertem Oracle ableiten.

<a id="s111"></a>
## S111 — Root-AGENTS wird zwischen Queueing und Start geändert

**Ursprung:** governance; A6-Zustände: GV02, GV07. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s111.b01"></a>
### S111.B01 — Regelwechsel vor Start sichtbar gehalten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Geänderte Root-Bytes werden vor Start mit der gebundenen Revision verglichen; Widerspruch sperrt Wirkung.

**Warum bleibt oder endet der Zustand?** Halt bis zuständige Person konkrete neue Revision erlaubt; bloßes erneutes Queueing löst ihn nicht.

**Zugeordnete Residues:** [GVR003: Gebundenes Eingabepaket](residues.md#gvr003), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer sieht beide belegten Revisionen und entscheidet über den erhaltenen Auftrag.

**Zu prüfen:** Start liest andere Bytes als das Paket oder Gate lässt die alte Zustimmung ungeprüft gelten.

<a id="s111.b02"></a>
### S111.B02 — Bindungszeitpunkt historisch unbekannt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Caller-Caching und Verbrauch wurden nicht aufgezeichnet; jetzige Root-Datei allein ist verfügbar.

**Warum bleibt oder endet der Zustand?** Heutige Dateiversion entscheidet nicht, was damals galt oder verbraucht wurde.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Bekannte Taskzeiten bleiben prüfbar; alte Regelbindung kann daraus nicht erfunden werden.

**Zu prüfen:** Zeitgenössischer belegter Cache oder Verbrauch klärt die gewählte Revision.

**Architekturfolge für diesen Stressor:** Zeitpunkt von Queueing, Kompilierung und Nutzung erfassen und festlegen, welche Regelrevision die Freigabe bindet; compile liest nur den Aufrufzeitpunkt.

<a id="s112"></a>
## S112 — Ein AGENTS-Quellfile ist plötzlich unlesbar

**Ursprung:** governance; A6-Zustände: GV03, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s112.b01"></a>
### S112.B01 — Unlesbare Pflichtquelle hält Kompilierung an

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Betroffene AGENTS-Datei ist explizit enthalten und Caller propagiert compile-Fehler statt Fallback.

**Warum bleibt oder endet der Zustand?** Lesefehler besteht bis rechtmäßiger Zugriff wiederhergestellt oder Auftrag geändert wird.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Operator kann Aufgabenmetadaten und den Quellfehler untersuchen, sofern separat lesbar; fehlende Anweisung bleibt unbekannt.

**Zu prüfen:** Caller liefert einen erfolgreichen Kontext trotz des enthaltenen unlesbaren Files.

<a id="s112.b02"></a>
### S112.B02 — Zulässiges gebundenes Paket bleibt verwendbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein zuvor vollständig erfasstes Paket ist lesbar, seine Verwendung weiterhin erlaubt und unabhängig vom defekten Quellpfad.

**Warum bleibt oder endet der Zustand?** Quellstörung verhindert diese gebundene Ausführung nicht; spätere neue Revisionen brauchen neue Prüfung.

**Zugeordnete Residues:** [GVR003: Gebundenes Eingabepaket](residues.md#gvr003)

**Was bleibt warum nutzbar?** Berechtigter Worker kann belegte Eingaben lesen; nicht automatisch auf beliebigen alten Cache übertragbar.

**Zu prüfen:** Regelvertrag verlangt die aktuell unlesbare Revision statt des erhaltenen Pakets.

**Architekturfolge für diesen Stressor:** UnreadableSource am Compiler eng zuschreiben; Caller darf Fehler nicht durch unerklärten alten Kontext ersetzen. Auftragsakte bleibt getrennt vom fehlenden Quellinhalt.

<a id="s113"></a>
## S113 — Human schreibt Geheimnis in Taskprompt

**Ursprung:** governance; A6-Zustände: GV19, GV08. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s113.b01"></a>
### S113.B01 — Geheimnis verbleibt in erlaubter Kopiengrenze

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Prompt wurde gespeichert, aber nicht unberechtigt gelesen; Kopienverzeichnis selbst ist zulässig und Nutzungswege sind gesperrt.

**Warum bleibt oder endet der Zustand?** Disposition dauert bis zuständige Entscheidung alle bekannten Kopien erfasst; Rotation löscht keine Bytes.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Datenverantwortliche können erlaubte Behandlung der konkreten Kopien bestimmen; bloße Speicherexistenz wird nicht als Leak gezählt.

**Zu prüfen:** Eine relevante Kopie fehlt im Verzeichnis oder ein unberechtigter Leser hat bereits Zugriff genommen.

<a id="s113.b02"></a>
### S113.B02 — Promptgeheimnis wurde gelernt

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Provider oder anderer Empfänger außerhalb der erlaubten Leser hat das konkrete Geheimnis erhalten.

**Warum bleibt oder endet der Zustand?** Spätere Löschung kann historische Kenntnis nicht beseitigen; Credentialrotation ändert nur künftige Macht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für nie erfolgte Offenlegung dieses Inhalts bleibt nichts nutzbar; daraus folgt nicht Verlust sämtlicher Aufgaben.

**Zu prüfen:** Alle Empfänger waren berechtigt oder konnten den Inhalt nicht entschlüsseln.

**Architekturfolge für diesen Stressor:** Secretannahme vor Persistenz und Weitergabe begrenzen; bei bereits gespeichertem Prompt konkrete Kopien/Empfänger erfassen, kein rückwirkendes Geheimhaltungsversprechen.

<a id="s114"></a>
## S114 — Knowledge-Link führt zyklisch durch tausende Notizen

**Ursprung:** governance; A6-Zustände: GV01, GV10, GV12. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s114.b01"></a>
### S114.B01 — Compiler läuft ohne Linkverfolgung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Aufgabe ist aus expliziten Quellen vollständig lösbar und Ergebnis unabhängig abgenommen.

**Warum bleibt oder endet der Zustand?** compile folgt den zyklischen Links nicht; endliche Aufgabe endet normal.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002)

**Was bleibt warum nutzbar?** Empfänger nutzt das abgenommene Ergebnis; Quellen-Nichtverfolgung allein hätte keine Nützlichkeit bewiesen.

**Zu prüfen:** Aufgabe benötigt in Wahrheit nicht gelesenen Linkinhalt.

<a id="s114.b02"></a>
### S114.B02 — Separater Crawler verbraucht seine begrenzte Zulassung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Crawler folgt Links, aber jeder Besuch wird im begrenzten Arbeitsregister gezählt und erneute Besuche bleiben dieselbe Identität.

**Warum bleibt oder endet der Zustand?** Restmenge wird ausgeschöpft oder abgeschlossen; Zyklen erzeugen keine ungezählten Wurzeln.

**Zugeordnete Residues:** [GVR013: Begrenztes Register offener Arbeit](residues.md#gvr013)

**Was bleibt warum nutzbar?** Operator kann die verbleibende endliche Suchmenge prüfen und bewusst erweitern; vollständige Wissenserschließung ist nicht garantiert.

**Zu prüfen:** Linkwechsel oder Retry schafft Besuche außerhalb desselben Limits.

**Architekturfolge für diesen Stressor:** Explizite Compilerquellen nicht mit rekursivem Knowledge-Crawler verwechseln. Ein späterer Crawler braucht eigene begrenzte Besuchs-/Arbeitsmenge.

<a id="s115"></a>
## S115 — Zwei Quellen widersprechen sich über aktuelle Freigaberegel

**Ursprung:** governance; A6-Zustände: GV15, GV02, GV05, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s115.b01"></a>
### S115.B01 — Widerspruch hält konkreten Effekt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Quellen betreffen denselben Effekt und Zeitraum; keine anerkannte Vorrangregel, wirksame Effektsperre vorhanden.

**Warum bleibt oder endet der Zustand?** Wiederholte widersprechende Aussagen liefern keine neue Zuständigkeit.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Zuständige Prüfer behalten die strittige Effekthülle; sachfremde Arbeit wird dadurch nicht automatisch gestoppt.

**Zu prüfen:** Eine bereits anerkannte Vorrangregel löst den konkreten Konflikt oder der Effekt läuft trotz Sperre.

<a id="s115.b02"></a>
### S115.B02 — Geltungsbereiche lösen den Scheinwiderspruch

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Anerkannte Ordnung weist Quellen unterschiedlichen Zeiten oder Zuständigkeiten zu; heutige Person ist befugt.

**Warum bleibt oder endet der Zustand?** Eindeutige Zuordnung beendet genau diesen Konflikt, nicht mögliche Outcome-Unklarheit.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032)

**Was bleibt warum nutzbar?** Befugte Person kann die richtige aktuelle Regel anwenden, ohne Quellmehrheit zu simulieren.

**Zu prüfen:** Beide Regeln gelten weiterhin gleichzeitig unvereinbar für denselben Effekt.

**Architekturfolge für diesen Stressor:** Freigabequellen nach Geltungsbereich und Wirksamkeitszeit ordnen; Mehrheit und letzte Ankunft sind ohne anerkannte Regel kein Vorrang.

<a id="s116"></a>
## S116 — Quellwebsite ändert Inhalt ohne URLänderung

**Ursprung:** governance; A6-Zustände: GV07, GV05, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s116.b01"></a>
### S116.B01 — Historische Webseite im Paket erhalten

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor Änderung wurden konkrete relevante Quellbytes mit belastbarem Zeit-/Versionsbezug legal erhalten.

**Warum bleibt oder endet der Zustand?** URLänderung berührt dieses historische Paket nicht; aktuelle Nutzung braucht neue Gültigkeitsprüfung.

**Zugeordnete Residues:** [GVR003: Gebundenes Eingabepaket](residues.md#gvr003)

**Was bleibt warum nutzbar?** Prüfer rekonstruiert die damalige Eingabe ohne heutige Website.

**Zu prüfen:** Paket enthält nur URL oder Zeitbezug ist frei erfunden.

<a id="s116.b02"></a>
### S116.B02 — Alte Webseitenfassung fehlt

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Es gibt nur veränderliche URL und heutige Seite, keine bekannte gesetzlich nutzbare alte Fassung.

**Warum bleibt oder endet der Zustand?** Wiederholtes Abrufen derselben URL erzeugt kein Wissen über frühere Bytes.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für exakte damalige Quellbytes besteht keine Restfähigkeit unter dieser Inventargrenze; heutige Aufgaben können trotzdem funktionieren.

**Zu prüfen:** Eine rechtmäßige zeitgenössische Fassung wird aufgefunden.

**Architekturfolge für diesen Stressor:** URL als Adresse, nicht Version führen. Rechtmäßig erfasste historische Quellbytes erlauben historische Aussagen, nicht automatische heutige Wahrheit.

<a id="s117"></a>
## S117 — Repo enthält bösartige native Harnesskonfiguration

**Ursprung:** governance; A6-Zustände: GV09, GV08, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s117.b01"></a>
### S117.B01 — Konfiguration bleibt vor Ausführung abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Harness behandelt fremde native Konfiguration nicht als automatische Befugnis; Freigabe vermittelt sämtliche Hooks.

**Warum bleibt oder endet der Zustand?** Ohne konkrete Prüfung bleibt Ausführung gesperrt; bloße Repoanwesenheit öffnet sie nicht.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer bearbeitet eine gehaltene Aktivierungsanforderung statt bereits ausgeführten Schadcodes.

**Zu prüfen:** Ein Install-/Start-Hook läuft vor der Entscheidung.

<a id="s117.b02"></a>
### S117.B02 — Harness kompromittiert, getrennter Bereich nutzbar

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Native Konfiguration startet Schadcode; benannter anderer Datenbereich liegt nachweislich außerhalb seiner Rechte.

**Warum bleibt oder endet der Zustand?** Angreifer erhält Kontrolle solange sein Zugang gilt; Isolation erhält nur den unbetroffenen Bereich.

**Zugeordnete Residues:** [GVR008: Außerhalb des Angreiferzugriffs liegender Datenbereich](residues.md#gvr008)

**Was bleibt warum nutzbar?** Berechtigter Besitzer verwendet isolierte Daten weiter; Aussagen und Geheimnisse im kompromittierten Bereich sind nicht geschützt.

**Zu prüfen:** Schadcode kann mit denselben Rechten auch den angeblich getrennten Bereich lesen.

**Architekturfolge für diesen Stressor:** Native Harnesskonfiguration ist eigene Ausführungsquelle außerhalb compile. Vor Hooks prüfen und reale Dateirechte begrenzen; Schreibverbot für Factory schützt nicht vor Harness-Leseverhalten.

<a id="s118"></a>
## S118 — Agent schreibt Shared Memory im Namen fremden Scopes

**Ursprung:** governance; A6-Zustände: GV02, GV28, GV05, GV08. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s118.b01"></a>
### S118.B01 — Fremde Schreibidentität abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Sitzung ist außerhalb des behauptenden Agents an Scope gebunden; Schreibpfad erzwingt diese Grenze.

**Warum bleibt oder endet der Zustand?** Auftrag bleibt gehalten bis korrekt autorisierte Identität vorliegt.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer sieht den abgewiesenen Schreibauftrag; bestehender fremder Bestand bleibt unverändert.

**Zu prüfen:** Direktzugriff umgeht die Bindung oder foreign-scope write wird akzeptiert.

<a id="s118.b02"></a>
### S118.B02 — Gefälschte Autorenschaft übernommen

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Store akzeptiert fremden Namen; unveränderte ursprüngliche Quellen sind separat erhalten.

**Warum bleibt oder endet der Zustand?** Die falsche Zeile beweist keine Autorenschaft; ohne weitere Wiederverwendung ist kein Attraktor begründet.

**Zugeordnete Residues:** [GVR003: Gebundenes Eingabepaket](residues.md#gvr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann Aussagen mit erhaltenen Eingabepaketen vergleichen; diese heilen nicht automatisch den gefälschten Eintrag.

**Zu prüfen:** Alle unabhängigen Originale sind ebenfalls manipuliert oder tatsächliche Identität ist anderweitig bewiesen.

**Architekturfolge für diesen Stressor:** Shared-Memory-Autorenschaft aus vertrauenswürdig gebundener Sitzung ableiten, nicht aus Parameter scope. Herkunft, tatsächlicher Leser und resultierende Wissensannahme separat prüfen.

<a id="s119"></a>
## S119 — Artefaktnamen unterscheiden sich nur durch Unicode-Normalisierung

**Ursprung:** governance; A6-Zustände: GV02, GV07, GV18, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s119.b01"></a>
### S119.B01 — Ähnliche Namen, getrennte Inhalte

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Originale liegen unter kollisionsfreien Objekt-IDs; Auswahl über normalisierten Namen wird bei Mehrdeutigkeit angehalten.

**Warum bleibt oder endet der Zustand?** Prüfer muss konkretes Objekt wählen, dann endet Auswahlhalt.

**Zugeordnete Residues:** [GVR037: Eindeutig adressierbarer Artefaktbestand](residues.md#gvr037)

**Was bleibt warum nutzbar?** Prüfer kann beide Bytes getrennt lesen und benennen; kein Original wird aus Displaygleichheit ersetzt.

**Zu prüfen:** Export oder Dateisystem vereinigt die Objekte doch zu einer überschreibenden Adresse.

<a id="s119.b02"></a>
### S119.B02 — Einziger Originalinhalt überschrieben

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Normalisierung führt zum Überschreiben der letzten Kopie; keine rechtmäßig rekonstruierbare Darstellung enthält die verlorenen Bytes.

**Warum bleibt oder endet der Zustand?** Namensmanifest kann den gelöschten Inhalt nicht wiederherstellen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für dieses Original bleibt kein Residue; das andere Artefakt und restliches System können bestehen.

**Zu prüfen:** Eine unabhängige Version enthält die ursprünglichen Bytes.

**Architekturfolge für diesen Stressor:** Artefakt-ID, Unicode-Anzeigename und Dateisystempfad trennen. Kollisionen vor Überschreiben prüfen; exakte Bytes sind noch keine fachliche Abnahme.

<a id="s120"></a>
## S120 — Context wird zum Debuggen vollständig in Telemetrie kopiert

**Ursprung:** governance; A6-Zustände: GV19, GV08, GV17. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s120.b01"></a>
### S120.B01 — Vollkontext bleibt eingeschränkt disponierbar

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Kopie existiert, nur befugte Leser haben Zugang und erlaubtes Verzeichnis erfasst Telemetrie samt Exporten.

**Warum bleibt oder endet der Zustand?** Nutzungs-/Retentionentscheidung bleibt offen bis geltende Zwecke geklärt sind.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Verantwortliche können konkrete Kopien sperren oder rechtmäßig behandeln; die Kopie selbst ist kein Resilienzgewinn.

**Zu prüfen:** Unbekannter Telemetrieexport oder unberechtigter Leser liegt außerhalb der vermittelten Grenze.

<a id="s120.b02"></a>
### S120.B02 — Telemetryleser kennt unzulässigen Kontext

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Ein nicht berechtigter Diagnoseempfänger hat den kopierten Inhalt tatsächlich gelesen.

**Warum bleibt oder endet der Zustand?** Lokale Reduktion der Telemetrie macht vergangene Offenlegung nicht rückgängig.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Vertraulichkeit dieses offengelegten Inhalts bleibt keine Restfähigkeit.

**Zu prüfen:** Empfänger war für genau diese Inhalte berechtigt oder bekam sie nie lesbar.

**Architekturfolge für diesen Stressor:** Telemetry nicht als Inhaltsbackup nutzen. Rohkontextkopien, Leserrechte und spätere Löschpflicht getrennt inventarisieren; die ADR-Regel allein beweist keine Filterung.

<a id="s121"></a>
## S121 — Plugin läuft als gleicher User und liest Keychain außerhalb Kernel

**Ursprung:** governance; A6-Zustände: GV08, GV09. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s121.b01"></a>
### S121.B01 — Gelesenes Keychaingeheimnis nicht rückholbar

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Unberechtigtes Plugin liest das konkrete Item erfolgreich wie in der Karte vorausgesetzt.

**Warum bleibt oder endet der Zustand?** Vergangene Kenntnis bleibt; spätere Rotation kann nur zukünftige Authentisierung entwerten.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die nie erfolgte Offenlegung dieses Items gibt es keinen Rest. Ein Audit über Kernelaufrufe ist kein vollständiges Zugriffsprotokoll.

**Zu prüfen:** OS verweigert den Itemzugriff oder es handelt sich nicht um unberechtigten Leser.

<a id="s121.b02"></a>
### S121.B02 — Andere Verwahrgrenze bleibt unberührt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Gelesenes Item ist kompromittiert, weitere konkret benannte Daten/Schlüssel liegen unter unabhängigen nicht zugänglichen Rechten.

**Warum bleibt oder endet der Zustand?** Fortgesetzter Pluginzugriff kann seine ursprüngliche Grenze nutzen, nicht die nachgewiesen getrennte.

**Zugeordnete Residues:** [GVR008: Außerhalb des Angreiferzugriffs liegender Datenbereich](residues.md#gvr008)

**Was bleibt warum nutzbar?** Berechtigter Besitzer nutzt ausschließlich die tatsächlich isolierten Daten; gleicher UID allein genügt nicht.

**Zu prüfen:** Zugang zum gelesenen Item vermittelt auch Zugriff auf die behauptet unabhängigen Schlüssel.

**Architekturfolge für diesen Stressor:** Keychainzugriff außerhalb Kernel als tatsächliche OS-Vertrauensgrenze behandeln. Erfolgreichen Lesezugriff nicht durch behauptete API-Autorisierung wegmodellieren.

<a id="s122"></a>
## S122 — Angreifer ersetzt Pluginbinary nach Manifestprüfung

**Ursprung:** governance; A6-Zustände: GV09, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s122.b01"></a>
### S122.B01 — Pfadtausch erreicht gestartetes Objekt nicht

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Loader startet nur das zuvor gebundene geprüfte Objekt oder verweigert nach Tausch; kein Hook läuft vorher.

**Warum bleibt oder endet der Zustand?** Substitution bleibt wirkungslos solange Objektbindung und Vertrauenswurzel gelten.

**Zugeordnete Residues:** [GVR009: Geprüftes ausführbares Objekt](residues.md#gvr009)

**Was bleibt warum nutzbar?** Loader kann authentisches Objekt nutzen und Prüfer den Tausch erklären; keine heutige Implementierung behauptet.

**Zu prüfen:** Der Prozess lädt ausgetauschte Haupt- oder Nebenbytes trotz erfolgreicher alter Prüfung.

<a id="s122.b02"></a>
### S122.B02 — Ausgetauschte Bytes bereits gestartet

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Angreiferbinary wurde mit relevanten Rechten gestartet; erreichbare Daten und ausgeführte Befehle sind noch unbekannt.

**Warum bleibt oder endet der Zustand?** Ohne unabhängige Ausführungs-/Zugriffsevidenz ist Schadensumfang nicht aus Manifeststatus ableitbar.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für saubere Ausführung fehlt ein belastbarer Restnachweis; Isolation oder externe Kopie wird nicht hinzuerfunden.

**Zu prüfen:** Unabhängige Ausführungsspur und Rechteinventar grenzen die tatsächlich berührten Objekte ein.

**Architekturfolge für diesen Stressor:** Geprüfte Bytes und gestartete Bytes einschließlich dynamischer Abhängigkeiten binden; bloßer Pfad oder Manifeststatus kann das Check-use-Fenster nicht schließen.

<a id="s123"></a>
## S123 — Transportplugin behauptet falschen authentifizierten Actor

**Ursprung:** governance; A6-Zustände: GV09, GV28, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s123.b01"></a>
### S123.B01 — Falscher Actor scheitert am unabhängigen Beleg

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Für konkreten Effekt existiert unabhängige persönliche Bindung; liegender Transport kann sie nicht erzeugen.

**Warum bleibt oder endet der Zustand?** Freigabe bleibt gesperrt, bis passende authentische Zustimmung vorliegt.

**Zugeordnete Residues:** [GVR010: Personengebundener Freigabebeleg](residues.md#gvr010), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Auditor prüft die echte Personen-/Effektbindung; Gate weist bloße Actorbehauptung zurück.

**Zu prüfen:** Transport kann selbst alle Belegbestandteile fälschen oder ein geteiltes Konto gilt als Person.

<a id="s123.b02"></a>
### S123.B02 — Actorhistorie ist nicht personenzuordenbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Nur Behauptung des lügenden Transports wurde erhalten, kein unabhängiger Personenbeleg.

**Warum bleibt oder endet der Zustand?** Nachträglicher erneuter Login beweist nicht, welcher Mensch damals zustimmte.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für den geforderten historischen Personenbeweis besteht keine Restfähigkeit aus dieser Datenlage; andere Fakten können lesbar sein.

**Zu prüfen:** Ein zeitgenössischer unabhängiger Beleg bindet genau diesen Effekt an die Person.

**Architekturfolge für diesen Stressor:** Transport ist Authentisierungs-Vertrauenswurzel oder muss unabhängige Personenbindung nachweisen. Kernel-Scopeprüfung einer Lüge liefert keinen Menschenbeleg.

<a id="s124"></a>
## S124 — Agent schreibt absichtlich direkt in SQLite

**Ursprung:** governance; A6-Zustände: GV28, GV27. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s124.b01"></a>
### S124.B01 — Direkte SQL-Manipulation lässt andere Grenze intakt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Agent schreibt DB erfolgreich; ein benannter anderer Datenbereich ist ihm tatsächlich nicht zugänglich.

**Warum bleibt oder endet der Zustand?** Manipulation kann fortdauern, solange Schreibrechte bestehen; kein Beweis eines selbstverstärkenden Kreislaufs.

**Zugeordnete Residues:** [GVR008: Außerhalb des Angreiferzugriffs liegender Datenbereich](residues.md#gvr008)

**Was bleibt warum nutzbar?** Besitzer nutzt nur unabhängig geschützte Daten; betroffene DB gilt nicht mehr als zuverlässige Autoritätsquelle.

**Zu prüfen:** Dieselben Rechte erreichen auch den als isoliert bezeichneten Bereich.

<a id="s124.b02"></a>
### S124.B02 — Einzelner Auditfakt außerhalb DB belegbar

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Betroffene Zeile wurde manipuliert, aber der konkrete frühere Fakt ist unabhängig verwahrt.

**Warum bleibt oder endet der Zustand?** Vergleich kann diese Abweichung aufklären, nicht sämtliche unverwahrten späteren Mutationen.

**Zugeordnete Residues:** [GVR033: Unabhängig verwahrter Tatsachenbeleg](residues.md#gvr033)

**Was bleibt warum nutzbar?** Auditor prüft den tatsächlich verwahrten Fakt trotz direktem SQLitewrite.

**Zu prüfen:** Beleg liegt unter derselben veränderbaren Schreibgrenze oder enthält den strittigen Fakt nicht.

**Architekturfolge für diesen Stressor:** Domain-Alleinschreiber erfordert OS-Schreibgrenze; Mutable-SQL-Constraints und Ziel-Eventlog nicht als Rootschutz ausgeben. Bei Bypass nur konkret unabhängige Fakten vertrauen.

<a id="s125"></a>
## S125 — SQL-/Pfadinjection steckt in neuem Pluginparameter

**Ursprung:** governance; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s125.b01"></a>
### S125.B01 — Parameter bleibt unschädliches Datum

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkreter Sink bindet SQL-Werte; Dateizugriffe lösen Objektidentität innerhalb erlaubter Grenze ohne Escape auf.

**Warum bleibt oder endet der Zustand?** Syntax hat keine Befehlswirkung; endliche Verarbeitung endet ohne fremde Mutation.

**Zugeordnete Residues:** [GVR008: Außerhalb des Angreiferzugriffs liegender Datenbereich](residues.md#gvr008)

**Was bleibt warum nutzbar?** Ein nachgewiesen nicht erreichbarer fremder Datenbereich bleibt für seinen Besitzer nutzbar; keine allgemeine Sicherheit aus einer Zeichenfolge.

**Zu prüfen:** Parameter erreicht SQL-Interpolation oder Pfadauflösung außerhalb dieser Rechte-/Objektgrenze.

<a id="s125.b02"></a>
### S125.B02 — Erreichbarer Sink verändert fremde Daten

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Sink interpoliert oder erlaubt Escape; fremdes Ziel ist schreibbar, aber genaue Zielobjekte und unveränderte Kopien sind unbekannt.

**Warum bleibt oder endet der Zustand?** Dauer und Schaden hängen vom ausgeführten Befehl und Schreibbereich ab, nicht vom Wort Injection.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für betroffene Integrität ist keine Restfähigkeit begründbar ohne Sink- und Dateninventar.

**Zu prüfen:** Konkreter Trace belegt Ablehnung oder begrenzt den tatsächlich veränderten Inhalt.

**Architekturfolge für diesen Stressor:** Offenen Sink zuerst bestimmen: gebundener SQL-Wert, aufgelöster erlaubter Pfad und effektive Rechte sind unterschiedliche Verträge. Kein pauschaler Injectionzustand.

<a id="s126"></a>
## S126 — Log enthält Terminalescapes die Diagnoseanzeige fälschen

**Ursprung:** governance; A6-Zustände: GV28, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s126.b01"></a>
### S126.B01 — Escapes werden als Daten gelesen

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Rohlog ist erhalten; unabhängiger Renderer stellt Steuerzeichen inert und begrenzt dar.

**Warum bleibt oder endet der Zustand?** Optischer Angriff hat in diesem Renderer keine Wirkung; Diagnose kann weitergehen.

**Zugeordnete Residues:** [GVR007: Inerte Diagnoseansicht](residues.md#gvr007)

**Was bleibt warum nutzbar?** Operator prüft die Originalzeichen, ohne dass sie andere Zeilen ersetzen.

**Zu prüfen:** Escape-Sequenz verändert sichtbare frühere Einträge oder führt eine Aktion aus.

<a id="s126.b02"></a>
### S126.B02 — Gefälschte Anzeige hat Entscheidung beeinflusst

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Unsicherer Renderer täuscht Operator, aber ursprüngliche Bytes bleiben unverändert erreichbar.

**Warum bleibt oder endet der Zustand?** Fehlentscheidung bleibt historisch möglich; spätere sichere Anzeige klärt nur den Darstellungsfehler.

**Zugeordnete Residues:** [GVR007: Inerte Diagnoseansicht](residues.md#gvr007)

**Was bleibt warum nutzbar?** Operator kann mit vertrauenswürdiger Ersatzansicht Rohbytes prüfen; daraus folgt keine Rücknahme eines ausgelösten Effekts.

**Zu prüfen:** Auch Rohbytes oder Ersatzrenderer sind manipuliert oder ausgelöste Wirkung bleibt unbeobachtet.

**Architekturfolge für diesen Stressor:** Terminaldarstellung und Ursprungsintegrität separat machen; neutrale Rohbyteansicht schützt nur vor Anzeige-Steuerung, nicht vor erfundenen Logfakten.

<a id="s127"></a>
## S127 — Altes gestohlenes Capability-Token wird nach Restore erneut gültig

**Ursprung:** governance; A6-Zustände: GV43, GV09, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s127.b01"></a>
### S127.B01 — Altes Token bleibt nach Restore abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein ausdrücklich verwaltetes maßgebliches Widerrufsregister außerhalb des zurückgesetzten Factory-Stores ist aktuell und authentisch abfragbar. Der Autoritätsadapter bindet Antwort an Principal Scope Grantrevision und Gültigkeit; der letzte kontrollierte Zulassungspfad weist das alte Token anhand dieses tatsächlich verfügbaren Stands ab.

**Warum bleibt oder endet der Zustand?** Restore ändert Aufgabenhistorie, nicht die erforderliche aktuelle Autoritätsepoche.

**Zugeordnete Residues:** [GVR011: Nicht zurückgesetzter Widerrufsstand](residues.md#gvr011)

**Was bleibt warum nutzbar?** M02 nutzt den tatsächlich verfügbaren aktuellen Stand aus dem unabhängig verwalteten Register über M16. M01 liefert nur historische lokale Fakten. Der Zweig behauptet GVR011 nicht bei ausgefallener Gegenquelle; frühere Wirkungen bleiben getrennt.

**Zu prüfen:** Restaurierter Snapshot setzt auch die einzige geprüfte Widerrufsquelle zurück.

**Nachtrag des Koordinators:** [A7S07](../review-dispositions.md#a7s07); ursprüngliche Abgabe unverändert.

<a id="s127.b02"></a>
### S127.B02 — Token wieder gültig, Gegenwart nicht belegt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Restore enthält nur alten Grant und keine aktuelle Gegenquelle; Angreifer kann ihn erneut vorlegen.

**Warum bleibt oder endet der Zustand?** Lokale Zustimmung bleibt veraltet bis unabhängige frische Evidenz erreichbar ist.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Prüfer liest historischen Grant mit Snapshotgrenze; daraus folgt kein heutiges Recht und kein Nachweis bereits erfolgter Ausnutzung.

**Zu prüfen:** Aktueller Widerruf wird vor Annahme tatsächlich geprüft.

<a id="s127.b03"></a>
### S127.B03 — Gegenquelle fehlt und eine konkrete neue Handlung bleibt gehalten

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Der alte lokale Grant ist lesbar, aber aktueller externer Widerrufsstand ist nicht verfügbar oder nicht frisch belegbar. Eine konkrete neue Effekthülle ist rechtmäßig erhalten und die vollständige Vermittlung hält sie vor Annahme an, statt den alten Grant als aktuell zu verwenden.

**Warum bleibt oder endet der Zustand?** Die Handlung bleibt bis zu tragfähiger aktueller Befugnis ungeklärt und gesperrt. Aus dem Halt entsteht kein Wissen darüber, ob und wann extern widerrufen wurde.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Historischer Bestand und konkrete gehaltene Absicht sind nutzbar. Der aktuelle Widerrufsstand GVR011 ist in diesem Zweig ausdrücklich nicht verfügbar.

**Zu prüfen:** Nach altem Snapshot einen Widerruf setzen und anschließend die unabhängige Quelle unerreichbar machen: neue vermittelte Handlung bleibt aus, aber kein aktueller Widerrufsbeleg wird erfunden.

**Nachtrag des Koordinators:** [A7S07](../review-dispositions.md#a7s07); ursprüngliche Abgabe unverändert.

**Architekturfolge für diesen Stressor:** Widerrufsevidenz außerhalb des restaurierten Vertrauensstands verankern; Ablaufzeit braucht verlässliche Zeitbasis und ist nicht Ersatz für aktuelle Rücknahme.

<a id="s128"></a>
## S128 — Bekannte Integration wird per Dependency-Typosquatting ersetzt

**Ursprung:** governance; A6-Zustände: GV09, GV08, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s128.b01"></a>
### S128.B01 — Namensähnliches Paket wird vor Code abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Auflösung ist an authentisch erhaltenes Paket gebunden und prüft vor jedem Installhook.

**Warum bleibt oder endet der Zustand?** Falscher Name kann ohne passende Byte-/Herkunftsbindung nicht aktiv werden.

**Zugeordnete Residues:** [GVR009: Geprüftes ausführbares Objekt](residues.md#gvr009)

**Was bleibt warum nutzbar?** Loader behält den authentischen konkreten Integrationssatz als nutzbaren Gegenstand.

**Zu prüfen:** Transitive Abhängigkeit oder Hook aus dem Typosquat läuft vor der Bindungsprüfung.

<a id="s128.b02"></a>
### S128.B02 — Typosquat aktiv, anderer Bereich bleibt getrennt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Schadpaket läuft; benannter legitimer Datenbereich liegt nachweislich außerhalb seiner effektiven Rechte.

**Warum bleibt oder endet der Zustand?** Angreiferzugang dauert mit dem Paket, nicht automatisch über seine Grenzen hinaus.

**Zugeordnete Residues:** [GVR008: Außerhalb des Angreiferzugriffs liegender Datenbereich](residues.md#gvr008)

**Was bleibt warum nutzbar?** Besitzer kann isolierten Bestand weiter nutzen; betroffene Integration und deren Ausgaben gelten nicht als sauber.

**Zu prüfen:** Installrechte umfassen auch den isoliert behaupteten Bestand.

**Architekturfolge für diesen Stressor:** Dependencyidentität und vorgezogene Installhooks in Aktivierungsvertrag aufnehmen. Vertrauter Integrationsname ist kein identisches Paket.

<a id="s129"></a>
## S129 — Ransomware verschlüsselt DB Backups und Knowledge gleichzeitig

**Ursprung:** governance; A6-Zustände: GV04, GV31, GV48, GV30. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s129.b01"></a>
### S129.B01 — Benannter unabhängiger Satz stellt Inhalt wieder her

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Eine vorab vorhandene rechtmäßige Kopie samt Schlüsseln und Decoder liegt außerhalb des Verschlüsselungszugriffs.

**Warum bleibt oder endet der Zustand?** Wiederherstellung endet, wenn Inhalt auf sauberer Umgebung lesbar ist; alte unbekannte Außenwirkungen bleiben gesondert.

**Zugeordnete Residues:** [GVR012: Vollständig nutzbarer Wiederherstellungssatz](residues.md#gvr012)

**Was bleibt warum nutzbar?** Berechtigter Wiederhersteller nutzt nur den konkret unabhängig erhaltenen Satz; keine spontane externe Kopie wird angenommen.

**Zu prüfen:** Drill ohne betroffene DB/Backups kann den erforderlichen Inhalt nicht lesen.

<a id="s129.b02"></a>
### S129.B02 — Kein bekannter Entschlüsselungsweg

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Alle bekannten Inhaltssätze sind verschlüsselt und kein verfügbarer Schlüssel oder Klartextpfad hilft; Systemspuren existieren.

**Warum bleibt oder endet der Zustand?** Wiederholter Restore derselben Bytes liefert keinen Schlüssel; dauerhafte Unmöglichkeit ist damit nicht bewiesen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für aktuelle Inhaltwiederherstellung fehlt ein nutzbarer Rest; Ciphertext und Erinnerungen schließen pauschalen Totalverlust aus.

**Zu prüfen:** Ein tatsächlich verfügbarer Schlüssel oder unabhängiger Inhaltspfad ermöglicht Lesen.

**Architekturfolge für diesen Stressor:** Ransomwaregrenze nach Daten, Schlüsseln und Tools schließen; Backupanzahl und vorhandenes Manifest beweisen keine außerhalb liegende Wiederherstellung.

<a id="s130"></a>
## S130 — Task erzeugt riesige gültige Delegationsketten ohne Zyklus

**Ursprung:** governance; A6-Zustände: GV10, GV01, GV11. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s130.b01"></a>
### S130.B01 — Großer endlicher Arbeitsbaum wird abgearbeitet

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Scopezahl und Wurzeln sind endlich, Lineage bleibt erhalten und verbleibende zugelassene Arbeit ist gezählt.

**Warum bleibt oder endet der Zustand?** Baum kann lange saturieren, aber Restverpflichtungen sinken bei Abschluss; keine unendliche Rekursion.

**Zugeordnete Residues:** [GVR013: Begrenztes Register offener Arbeit](residues.md#gvr013)

**Was bleibt warum nutzbar?** Scheduler nutzt das endliche Arbeitsregister zur Abweisung und Abarbeitung, nicht nur den Zyklustest.

**Zu prüfen:** Neue ungezählte Wurzeln entstehen oder Abschlüsse vermindern Restmenge nicht.

<a id="s130.b02"></a>
### S130.B02 — Rückstau erzeugt neue Wurzeln

**Art:** eskalation. **Residue-Status:** teilweise.

**Voraussetzungen:** Verzögerungen lösen autorisierte frische Retry-/Delegationswurzeln aus; Reproduktion liegt über Abbau und Gesamtlimit fehlt.

**Warum bleibt oder endet der Zustand?** Rückstau → neue Wurzeln → mehr Rückstau → weitere neue Wurzeln; Ressourcenende kann Regime abbrechen.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Erhaltene Aufgabenlinien bleiben untersuchbar; sie begrenzen die Last nicht.

**Zu prüfen:** Bei gleicher deklarierter Basislast nach Ende des Startfehlers verschwindet Rückstau dauerhaft; Nullankunftstest ist getrennt auszuwerten.

**Nachtrag des Koordinators:** [A7S04](../review-dispositions.md#a7s04); ursprüngliche Abgabe unverändert.

**Architekturfolge für diesen Stressor:** Endliche Scopekette und Gesamtmenge trennen; Wurzelerzeugung, Retryidentität und alle Produzenten gemeinsam begrenzen.

<a id="s131"></a>
## S131 — Betroffene Person verlangt Löschung aus append-only History

**Ursprung:** governance; A6-Zustände: GV17, GV18, GV19. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s131.b01"></a>
### S131.B01 — Disposition unter geklärtem Prüfvorbehalt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Anfrage gilt möglicherweise; Rechtsprüfung und erlaubtes Kopien-/Zweckverzeichnis sind vorhanden, Nutzung wird begrenzt.

**Warum bleibt oder endet der Zustand?** Hold dauert bis konkrete zulässige Datenbehandlung feststeht, nicht bis Replay erfolgreich ist.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Berechtigte Verantwortliche können betroffene Kopien und Handlungen bestimmen; Verzeichnis darf nicht selbst verbotene Daten retten.

**Zu prüfen:** Anordnung verlangt auch sofortige Löschung des Verzeichnisses oder relevante Kopien sind unbekannt.

<a id="s131.b02"></a>
### S131.B02 — Erlaubte Herkunft bleibt, gelöschte Payload fehlt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Indispensable Payload wurde rechtmäßig vollständig gelöscht; ausgewählte Herkunftskanten dürfen bleiben.

**Warum bleibt oder endet der Zustand?** Kein Feedback: fehlender Inhalt entsteht nicht aus erhaltenen Kanten.

**Zugeordnete Residues:** [GVR015: Erlaubte historische Herkunftskanten](residues.md#gvr015)

**Was bleibt warum nutzbar?** Auskunftsbearbeiter beantwortet nur Fragen aus erlaubten Kanten; für payloadabhängigen Beweis bleibt keines.

**Zu prüfen:** Die konkrete Anfrage ist ohne Payload nicht beantwortbar oder auch Kanten dürfen nicht bleiben.

**Architekturfolge für diesen Stressor:** Löschpflicht und zulässig notwendige Provenienz pro Datenobjekt bestimmen; append-only Ziel rechtfertigt weder unerlaubtes Behalten noch die Rekonstruktion gelöschter Inhalte.

<a id="s132"></a>
## S132 — Gericht verlangt Legal Hold während Retention löschbereit ist

**Ursprung:** governance; A6-Zustände: GV17, GV18. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s132.b01"></a>
### S132.B01 — Hold kommt vor letzter Löschung durch

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Gültige Anordnung ist vor relevantem Löschcommit wirksam, Daten und erlaubtes Verzeichnis bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Erhaltungsgebot hält Löschung an bis zuständige Freigabe; Verwendung ist separat beschränkt.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Verantwortliche können die konkret gehaltenen Kopien nachweisen und erlaubte Einsicht organisieren.

**Zu prüfen:** Ein Löschpfad entfernt betroffene Daten trotz vor Commit wirksamen Holds.

<a id="s132.b02"></a>
### S132.B02 — Letzte nötige Kopie vor Hold gelöscht

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Retention löscht einzige rekonstruierbare benötigte Kopie bevor Hold wirksam wird; keine zulässige weitere Darstellung existiert.

**Warum bleibt oder endet der Zustand?** Spätere Anordnung erzeugt keine verlorenen Bytes.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Erhaltung bzw. Wiederherstellung dieses Inhalts gibt es keinen Rest; sonstige Rechtsakten können bestehen.

**Zu prüfen:** Zeit-/Kopieninventar zeigt, dass Inhalt bei Holdwirksamkeit noch vorhanden und erreichbar war.

**Architekturfolge für diesen Stressor:** Legal-Hold-Wirksamkeit und Löschcommit auf dieselben Datenpfade beziehen. Backupfunktion ohne Retention belegt keine vorhandene atomare Holdbehandlung.

<a id="s133"></a>
## S133 — Kunde verlangt Datenresidenz die aktueller Modellprovider nicht erfüllt

**Ursprung:** governance; A6-Zustände: GV20, GV08, GV17, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s133.b01"></a>
### S133.B01 — Nicht konformer Providerpfad angehalten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Bindende Residenzregel schließt aktuellen Provider aus und sämtliche Sendepfade bleiben vor Annahme gesperrt.

**Warum bleibt oder endet der Zustand?** Halt bis erlaubter Pfad oder gültig geänderter Vertrag besteht.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Verantwortliche behalten den ungesendeten konkreten Auftrag zur zulässigen Neubewertung; eigenständige lokale Auftragserfüllung wird nicht vorausgesetzt.

**Zu prüfen:** Daten verlassen die Grenze über Telemetrie oder anderen nicht vermittelten Pfad.

<a id="s133.b02"></a>
### S133.B02 — Unzulässiger Ort ohne belegte Offenlegung

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Transfer an vertraglich verbotenen Ort erfolgt, aber kein neuer unberechtigter Klartextleser ist nachgewiesen; Verzeichnis ist erlaubt erhalten.

**Warum bleibt oder endet der Zustand?** Historische Ortsverletzung bleibt erfolgt; Behandlung hängt von zuständiger Vertrags-/Rechtsentscheidung ab.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Datenverantwortliche können konkrete Kopien/Orte und zulässige Folgeschritte untersuchen; keine Vertraulichkeitsvernichtung wird erfunden.

**Zu prüfen:** Transfer war nach tatsächlicher Regel erlaubt oder unberechtigtes Lesen wird separat nachgewiesen.

**Architekturfolge für diesen Stressor:** Residenzverletzung ohne neuen unberechtigten Leser ausdrücklich von Offenlegung unterscheiden. Providerpfad vor Sendung prüfen; Vertragsort ist kein kryptographischer Begriff.

<a id="s134"></a>
## S134 — Backup enthält Geheimnisse die im Original schon rotiert wurden

**Ursprung:** governance; A6-Zustände: GV19, GV08, GV43. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s134.b01"></a>
### S134.B01 — Historische Kopien kontrolliert disponierbar

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Alte Bytes existieren nur bei berechtigten Verwahrern; Inventar und Nutzungsbeschränkung sind erlaubt und vollständig.

**Warum bleibt oder endet der Zustand?** Kopien bleiben sensibel bis zulässige Disposition; ungültiger Login entwertet nicht jeden Inhalt.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Verantwortliche können erlaubte Behandlung der alten Kopien planen und prüfen, ohne einen Leak zu behaupten.

**Zu prüfen:** Ein nicht erfasster Export oder tatsächlicher unberechtigter Leser wird gefunden.

<a id="s134.b02"></a>
### S134.B02 — Restore reaktiviert alte Credentialmacht nicht

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kopie wird restauriert, aber unabhängiger aktueller Widerrufsstand vermittelt jede neue Zulassung.

**Warum bleibt oder endet der Zustand?** Alte Secretbytes bleiben historisch, gegenwärtige Macht wird separat abgewiesen.

**Zugeordnete Residues:** [GVR011: Nicht zurückgesetzter Widerrufsstand](residues.md#gvr011)

**Was bleibt warum nutzbar?** Gate kann neue Nutzung alter Zugangsdaten verhindern; gespeicherte private Informationen bleiben dadurch nicht gelöscht.

**Zu prüfen:** Widerrufsprüfung stammt allein aus derselben alten Kopie.

**Architekturfolge für diesen Stressor:** Backupgeheimnisse, derzeitige Credentialgültigkeit und Restore-Autorität getrennt führen; Rotation ist weder Löschung noch Schutz gegen zurückgesetzten Widerruf.

<a id="s135"></a>
## S135 — Lizenz einer notwendigen Library wird für neue Releases unbrauchbar

**Ursprung:** governance; A6-Zustände: GV25, GV20, GV21, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s135.b01"></a>
### S135.B01 — Erhaltene Version rechtmäßig weiter nutzbar

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Alte Nutzungsrechte bleiben gültig und vollständiger kompatibler Lauf-/Bausatz existiert.

**Warum bleibt oder endet der Zustand?** Neue Releases bleiben rechtlich blockiert, bisheriger eng begrenzter Betrieb kann andauern.

**Zugeordnete Residues:** [GVR016: Rechtmäßig nutzbarer Lauf- und Bausatz](residues.md#gvr016)

**Was bleibt warum nutzbar?** Maintainer kann genau die erhaltene rechtmäßige Version betreiben oder bauen.

**Zu prüfen:** Benötigte Sicherheits-/Plattformfunktion erfordert doch die unzulässige neue Version.

<a id="s135.b02"></a>
### S135.B02 — Erforderliche neue Nutzung bleibt verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Auftrag braucht die lizenzrechtlich ausgeschlossene neue Fassung und Freigabegrenze vermittelt deren Nutzung.

**Warum bleibt oder endet der Zustand?** Halt bis zulässiger Ersatz oder geänderter Auftrag; alte Lizenz löst neue Rechte nicht.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Berechtigter Entscheider hat den gehaltenen Auftrag, nicht die Fähigkeit zur unzulässigen Nutzung.

**Zu prüfen:** Ein nötiger Build-/Veröffentlichungspfad nutzt die verbotene Fassung dennoch.

**Architekturfolge für diesen Stressor:** Lizenzrechte des erhaltenen Versionssatzes ausdrücklich vom neuen Release trennen; keine rückwirkende Ungültigkeit und keine Ersatzlibrary ohne Kompatibilitäts-/Rechtebeleg annehmen.

<a id="s136"></a>
## S136 — Audit muss beweisen welcher Mensch konkret freigegeben hat

**Ursprung:** governance; A6-Zustände: GV03, GV28, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s136.b01"></a>
### S136.B01 — Konkrete Zustimmung personengebunden beweisbar

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Zeitgenössischer unabhängiger Beleg bindet identifizierte Person und damalige Befugnis an exakt diesen Effekt.

**Warum bleibt oder endet der Zustand?** Enges Audit endet mit belegter Zustimmung; Ausführung und heutige Autorität bleiben separate Fragen.

**Zugeordnete Residues:** [GVR010: Personengebundener Freigabebeleg](residues.md#gvr010)

**Was bleibt warum nutzbar?** Autorisierter Auditor kann die geforderte historische Zustimmung nachvollziehen.

**Zu prüfen:** Geteiltes Konto, unklarer Effekttext oder fälschbarer Transportactor ersetzt tatsächliche Personenbindung.

<a id="s136.b02"></a>
### S136.B02 — Nur Rollenname erhalten

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Historische Daten enthalten lediglich geteilte Rolle oder ungesicherte Actorbehauptung; kein unabhängiger persönlicher Beleg.

**Warum bleibt oder endet der Zustand?** Nachträgliche Befragung ohne belastbare Erinnerung liefert keinen sicheren damaligen Beweis.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für konkreten Menschenbeweis besteht aus diesem Inventar keine Restfähigkeit; die Taskhistorie ist nicht automatisch wertlos.

**Zu prüfen:** Zulässiger zeitnaher Beleg oder tragfähige unabhängige Erinnerung klärt genau diese Zustimmung.

**Architekturfolge für diesen Stressor:** Freigabebeweis muss Person, damalige Befugnis, Wirkung und Zeitpunkt binden; spätere Rollenauflösung und Transportname reichen nicht.

<a id="s137"></a>
## S137 — Behörde beschlagnahmt Originalgerät und fordert Einsicht

**Ursprung:** governance; A6-Zustände: GV32, GV17, GV08. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s137.b01"></a>
### S137.B01 — Original in amtlicher Verwahrung

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Gerät ist beschlagnahmt; erlaubtes Verzeichnis über Custody und Datenpflichten ist andernorts tatsächlich zugänglich.

**Warum bleibt oder endet der Zustand?** Behördliche Zugriffsbedingungen halten Einschränkung aufrecht, nicht ein interner Systemloop.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Berechtigte Rechtsverantwortliche können erlaubte Einsicht beantragen bzw. organisieren; freie Originalnutzung ist nicht erhalten.

**Zu prüfen:** Auch das Verzeichnis ist beschlagnahmt/unzulässig oder Zugriff bereits uneingeschränkt erlaubt.

<a id="s137.b02"></a>
### S137.B02 — Erlaubter unabhängiger Satz trägt begrenzten Betrieb

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkrete vorhandene Kopie, Schlüssel und Leser dürfen laut bindender Custodyregel genutzt werden.

**Warum bleibt oder endet der Zustand?** Wiederherstellung kann begrenzte Aufgaben freigeben; gerichtliche Einschränkungen bestehen fort.

**Zugeordnete Residues:** [GVR012: Vollständig nutzbarer Wiederherstellungssatz](residues.md#gvr012)

**Was bleibt warum nutzbar?** Berechtigter Wiederhersteller nutzt nur ausdrücklich erlaubten Satz, nicht heimlich kopiertes Original.

**Zu prüfen:** Custodyanordnung verbietet diese Nutzung oder Satz enthält nicht die nötigen Daten.

**Architekturfolge für diesen Stressor:** Custody, erlaubte Einsicht und Weiterbetrieb separat erfassen; keine Umgehung einer Beschlagnahme und keine unabhängige Kopie ohne benannte erlaubte Existenz.

<a id="s138"></a>
## S138 — Ein ursprünglich harmloses Feld wird später als sensibel eingestuft

**Ursprung:** governance; A6-Zustände: GV19, GV17, GV08. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s138.b01"></a>
### S138.B01 — Neuklassifizierte Kopien unter begrenzter Behandlung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kopien und Zwecke sind bekannt; ihre Metadaten dürfen erhalten bleiben und neue Nutzungssperren greifen.

**Warum bleibt oder endet der Zustand?** Neue Einstufung hält bisherige breite Nutzung an bis konkrete Disposition geklärt ist.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Verantwortliche können erlaubte aktuelle Behandlung der Feldkopien durchsetzen, ohne alte Leser pauschal zu illegalisieren.

**Zu prüfen:** Verdeckte Kopien oder Nebenpfade bleiben außerhalb der neuen Behandlung.

<a id="s138.b02"></a>
### S138.B02 — Neu verbotene Nutzung nicht rekonstruierbar

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Historische Empfänger-/Zweckdaten fehlen; Feld ist jetzt sensibel, aber konkretes damaliges oder heutiges Lesen unbekannt.

**Warum bleibt oder endet der Zustand?** Neue Einstufung allein füllt die fehlende Empfängerhistorie nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für eine vollständige Offenlegungs-/Zweckbewertung ist kein Restnachweis belegt; tatsächlicher Geheimhaltungsverlust bleibt separat offen.

**Zu prüfen:** Vollständige zulässige Empfänger-/Zweckbelege werden gefunden.

**Architekturfolge für diesen Stressor:** Klassifikation nach Wirksamkeitszeit, Zweck und tatsächlicher Kopie führen. Sensibilität heute beweist keinen vergangenen Rechtsbruch oder bereits erfolgtes Lesen.

<a id="s139"></a>
## S139 — Auskunftsanfrage verlangt Provenienz über gelöschte Scopes hinweg

**Ursprung:** governance; A6-Zustände: GV01, GV18, GV17, GV03. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s139.b01"></a>
### S139.B01 — Gelöschte Scopes über erlaubte Kanten beantwortbar

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Benötigte Beziehungen wurden unabhängig von Registryeinträgen legal erhalten und genügen genau dieser Anfrage.

**Warum bleibt oder endet der Zustand?** Eng begrenzte Auskunft endet mit belegten Kanten; keine Wiederbelebung gelöschter Scopes nötig.

**Zugeordnete Residues:** [GVR015: Erlaubte historische Herkunftskanten](residues.md#gvr015)

**Was bleibt warum nutzbar?** Auskunftsbearbeiter beantwortet die konkrete Herkunftsfrage aus vorhandener Struktur.

**Zu prüfen:** Anfrage benötigt eine nicht erhaltene Kante oder ursprünglichen Inhalt.

<a id="s139.b02"></a>
### S139.B02 — Erforderliche Herkunft endgültig im Inventar verloren

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Notwendige Beziehung wurde gelöscht und keine zulässige rekonstruierbare Darstellung oder Erinnerung enthält sie.

**Warum bleibt oder endet der Zustand?** Erneuter Export übriger Scopes erfindet die fehlende Beziehung nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für vollständige verlangte Herkunft gibt es keinen Rest; bekannte Teilbeziehungen dürfen nur als unvollständig ausgewiesen werden.

**Zu prüfen:** Eine rechtmäßig zugängliche unabhängige Herkunftskante schließt die Lücke.

**Architekturfolge für diesen Stressor:** Aktive Scoperegistry und zulässig aufbewahrte Herkunft trennen. Auskunftumfang muss fehlende Kanten und Payload ausdrücklich benennen.

<a id="s140"></a>
## S140 — Versicherer verlangt beweisbare Recovery statt vorhandener Backups

**Ursprung:** governance; A6-Zustände: GV31, GV47, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s140.b01"></a>
### S140.B01 — Backup vorhanden, Versicherungsbeweis offen

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Benannter kompletter Satz ist lesbar vorhanden, aber repräsentativer vollständiger Versuch für den Versicherungsanspruch fehlt.

**Warum bleibt oder endet der Zustand?** Inventar allein beendet Beweislücke nicht; Wiederholungsbehauptungen ersetzen keinen Versuch.

**Zugeordnete Residues:** [GVR012: Vollständig nutzbarer Wiederherstellungssatz](residues.md#gvr012)

**Was bleibt warum nutzbar?** Wiederhersteller kann den Satz prüfen; für garantierte Recovery liegt noch kein Testbeleg vor.

**Zu prüfen:** Ein repräsentativer abgenommener Versuch belegt bereits genau den geforderten Anspruch.

<a id="s140.b02"></a>
### S140.B02 — Enger Recoverynachweis trägt Auditantwort

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Tatsächlicher vollständiger Versuch ist unabhängig dokumentiert und deckt den konkret vereinbarten Schadens-/Zeitumfang.

**Warum bleibt oder endet der Zustand?** Audit endet nur innerhalb dieser Hülle; neue Abhängigkeiten eröffnen neue Prüfung.

**Zugeordnete Residues:** [GVR025: Begrenzter Wiederherstellungsnachweis](residues.md#gvr025)

**Was bleibt warum nutzbar?** Auditor nutzt die belegte Dauer und Inhaltsabnahme, nicht bloß die Dateianzahl.

**Zu prüfen:** Fehlender Schlüssel, Außenwirkung oder andere Ersatzhardware fällt außerhalb der behaupteten Testhülle.

**Architekturfolge für diesen Stressor:** Wiederherstellungsinventar und durchgeführten Nachweis als getrennte Objekte führen; Prüfziel einschließlich Inhalt, Schlüssel, Zeit und Außenwirkungsabgleich ausweisen.

<a id="s141"></a>
## S141 — Tokenpreise steigen über Nacht um Faktor hundert

**Ursprung:** governance; A6-Zustände: GV21, GV22, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s141.b01"></a>
### S141.B01 — Neue teure Aufrufe bleiben innerhalb Budget zurückgehalten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Neuer bindender Tarif ist bekannt, vollständige Ausgabenreservierung erkennt fehlenden Rahmen vor Annahme.

**Warum bleibt oder endet der Zustand?** Halt bis ausreichendes legitimes Budget oder günstiger erlaubter Pfad besteht.

**Zugeordnete Residues:** [GVR018: Wirksame Ausgabenreservierung](residues.md#gvr018), [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017)

**Was bleibt warum nutzbar?** Budgethalter stoppt neue Verpflichtungen und sieht alte bekannte Kosten; vergangene Aufrufe werden nicht ungeschehen.

**Zu prüfen:** Ein neuer Aufruf wird außerhalb Reservierung angenommen oder sein Preis hat keine bindende Obergrenze.

<a id="s141.b02"></a>
### S141.B02 — Hochwertiger Auftrag lohnt trotz Preissprung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Gesamter tatsächlicher Aufwand bleibt unter belegtem Auftragswert und erlaubtem Budget; Ergebnis wird abgenommen.

**Warum bleibt oder endet der Zustand?** Endlicher wertvoller Auftrag endet, allgemeine Wirtschaftlichkeit bleibt unbewiesen.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002), [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017)

**Was bleibt warum nutzbar?** Empfänger nutzt das Ergebnis und Budgethalter den konkreten Kostennachweis.

**Zu prüfen:** Spätere Rechnung oder menschlicher Nacharbeitsaufwand übersteigt die angenommene Kostengrenze.

**Architekturfolge für diesen Stressor:** Tarifwirksamkeit an Annahmezeit binden und zukünftiges Zulassen von bereits eingegangenen Kosten trennen. Faktor hundert bedeutet nicht automatisch Unwirtschaftlichkeit jedes Auftrags.

<a id="s142"></a>
## S142 — Provider ersetzt Prepaid durch unbekannte nachträgliche Abrechnung

**Ursprung:** governance; A6-Zustände: GV22, GV21. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s142.b01"></a>
### S142.B01 — Bekannte Nutzung, unbekannte Nachschuld

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Provider berechnet nachträglich ohne bekannte bindende Obergrenze; lokale Nutzung und Annahmezeiten bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Fortgesetzte Aufrufe erhöhen unbewertete Verpflichtung; erst Vertrag/Rechnung kann sie beziffern.

**Zugeordnete Residues:** [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017)

**Was bleibt warum nutzbar?** Budgethalter kann bekannte Nutzung und offenen Rechnungsanteil ausweisen, keine sichere Gesamtschranke.

**Zu prüfen:** Vollständiger bindender Tarif begrenzt bereits alle angenommenen Aufrufe.

<a id="s142.b02"></a>
### S142.B02 — Neue Exposition bis Vertragsklärung gesperrt

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Vollständige Effektsperre hält neue Provideranforderungen zurück; frühere Postpaidschuld bleibt unbekannt.

**Warum bleibt oder endet der Zustand?** Halt bis legitime bindende Bedingungen vorliegen oder Dienst aufgegeben wird.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005), [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017)

**Was bleibt warum nutzbar?** Entscheider behält offene Anfrage und bekannte Altverpflichtungen; Sperre löscht keine Schuld.

**Zu prüfen:** Weiterlaufender nicht vermittelter Worker erzeugt weitere abrechenbare Aufrufe.

**Architekturfolge für diesen Stressor:** Unbekanntes Postpaid nicht als prepaidbegrenztes Risiko darstellen. Ohne bindende Preisgrenze keine GVR018-Obergrenze behaupten; neue Exposition separat sperren.

<a id="s143"></a>
## S143 — Ein großer Scope monopolisiert alle Slots mit legalen Tasks

**Ursprung:** governance; A6-Zustände: GV10, GV23, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s143.b01"></a>
### S143.B01 — Großscope belastet, kleine Anteile bleiben erreichbar

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Legale Großlast dauert an; unabhängige verbindliche Anteile und Betriebsreserve werden vollständig durchgesetzt.

**Warum bleibt oder endet der Zustand?** Dauerlast hält Engpass, aber kann reservierte Anteile nicht übernehmen.

**Zugeordnete Residues:** [GVR019: Durchsetzbare Ressourcenanteile](residues.md#gvr019)

**Was bleibt warum nutzbar?** Kleine Scopes erhalten ihren konkreten Anteil; weder allgemeine Fristgarantie noch Gleichheit aller Ergebnisse folgt.

**Zu prüfen:** Messung zeigt kleinen Scope trotz erfüllter Vertragsbedingungen unter seinem zugesagten Anteil.

<a id="s143.b02"></a>
### S143.B02 — Auslastung erzeugt künftige Zuteilungsmacht

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Großscope bekommt wegen heutiger hoher Ausgabe weitere Slots; benachteiligte Scopes verlieren dadurch belegbaren Wert und Stimme.

**Warum bleibt oder endet der Zustand?** Mehr Slots → mehr sichtbare Leistung → mehr Zuteilung → weniger konkurrierende Wertbelege.

**Zugeordnete Residues:** [GVR004: Rohmessungen mit begrenztem Vergleich](residues.md#gvr004)

**Was bleibt warum nutzbar?** Erhaltene rohe Verbrauchs-/Ergebnisgrößen bleiben auswertbar, sofern unabhängig aufbewahrt; sie erzwingen keine faire Entscheidung.

**Zu prüfen:** Zuteilung ist unabhängig von vergangener Belegung und kleine Scopes erholen sich nach endlichem Burst.

**Architekturfolge für diesen Stressor:** Faire Ressourcenanteile getrennt von per-agent max_sessions und Organisationsfeedback prüfen; endlichen Großauftrag nicht allein zum Attraktor erklären.

<a id="s144"></a>
## S144 — Jeder kleine Task kostet mehr menschliche Freigabe als er spart

**Ursprung:** governance; A6-Zustände: GV21, GV16. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s144.b01"></a>
### S144.B01 — Unrentable Kleinarbeit wird legitim beendet

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Gesamtaufwand einschließlich Mensch ist belegt höher als Nutzen; zuständige Person verwirft weitere Aufträge wahrheitsgemäß.

**Warum bleibt oder endet der Zustand?** Bewusste Nichtnutzung beendet diesen Arbeitsstrom, ohne nützliche Lieferung zu behaupten.

**Zugeordnete Residues:** [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017), [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Verantwortliche behalten Aufwandbeleg und begründete Entscheidung für spätere Neubewertung.

**Zu prüfen:** Reale End-to-end-Bilanz zeigt doch positiven Wert oder Aufträge laufen verdeckt weiter.

<a id="s144.b02"></a>
### S144.B02 — Teurer Freigabeweg wird belohnt umgangen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Team kann Gate umgehen und erhält für schnellere sichtbare Lieferung mehr Belohnung als sichtbare Sanktion.

**Warum bleibt oder endet der Zustand?** Bypass → bessere Kennzahl → Belohnung → Vorbild für weiteren Bypass → unterdrückte Risiken.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Unveränderte Auftragsakte kann spätere Untersuchung tragen; sie bewahrt nicht die umgangene Autorität.

**Zu prüfen:** Unabhängig erfasste Folgen überwiegen Belohnung und Bypass sinkt bei gleicher Arbeitslast.

**Architekturfolge für diesen Stressor:** Menschliche Freigabekosten als tatsächlichen Aufwand führen. Legitime Nichtnutzung und autorisierten kleineren Vertrag von verstecktem Genehmigungsbypass trennen.

<a id="s145"></a>
## S145 — Nachfrage verzehnfacht sich ohne weitere Workspaces

**Ursprung:** governance; A6-Zustände: GV01, GV10, GV24, GV11. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s145.b01"></a>
### S145.B01 — Dauerlast wird begrenzt zugelassen

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Nachfrage übersteigt feste Workspaces, sämtliche Produzenten treffen begrenztes Arbeitsregister und zugesagte Ressourcenanteile.

**Warum bleibt oder endet der Zustand?** Abgewiesene Nachfrage bleibt außen; zugelassene Menge kann innerhalb realer Kapazität abgearbeitet werden.

**Zugeordnete Residues:** [GVR013: Begrenztes Register offener Arbeit](residues.md#gvr013), [GVR019: Durchsetzbare Ressourcenanteile](residues.md#gvr019)

**Was bleibt warum nutzbar?** Scheduler behält endliche Arbeit und konkrete Anteile; abgelehnte oder verspätete Aufträge sind kein Erfolg.

**Zu prüfen:** Ein Producer umgeht Zulassung oder zugesagter Anteil ist wegen gemeinsamer Ressource nicht verfügbar.

<a id="s145.b02"></a>
### S145.B02 — Echter Geschäftszeitpunkt verpasst

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Zugelassene Arbeit wird erst nach harter nicht verlängerbarer Frist fertig.

**Warum bleibt oder endet der Zustand?** Vergangener Zeitpunkt lässt sich durch spätere erfolgreiche Ausführung nicht wiederherstellen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für ursprüngliche Rechtzeitigkeit bleibt kein Rest; erhaltenes spätes Ergebnis wäre separat nach anderem Zweck abnehmbar.

**Zu prüfen:** Frist ist tatsächlich weich oder unverändert wertvolle Annahme erfolgt rechtzeitig.

**Architekturfolge für diesen Stressor:** Zehnfache Nachfrage gegen reale Kapazität, Deadlines und Retryidentität prüfen. Begrenzte Zulassung hält Arbeit lesbar, nicht alle angefragten Termine ein.

<a id="s146"></a>
## S146 — Budget wird während langer Agentarbeit auf null gesetzt

**Ursprung:** governance; A6-Zustände: GV21, GV22, GV13. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s146.b01"></a>
### S146.B01 — Neue Kosten ab Budgetnull vermittelt angehalten

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Bindende Preisgrenze und Ausgabengate greifen vor jedem neuen Aufruf; Altverpflichtungen sind gesondert erfasst.

**Warum bleibt oder endet der Zustand?** Halt besteht bis neue legitime Reservierung; alter angenommener Aufruf kann weiter Kosten verursachen.

**Zugeordnete Residues:** [GVR018: Wirksame Ausgabenreservierung](residues.md#gvr018), [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017)

**Was bleibt warum nutzbar?** Budgethalter stoppt nur noch nicht eingegangene Kosten und sieht bereits bekannte Exposition.

**Zu prüfen:** Worker kann nach Nullsetzung ohne neue Reservierung zusätzliche kostenpflichtige Aufrufe annehmen lassen.

<a id="s146.b02"></a>
### S146.B02 — Cancel gespeichert, tatsächlicher Verbrauch offen

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Agent quittiert keinen verlässlichen Stop oder Provideroutcome ist unbekannt; gespeicherte Versuche und bekannte Nutzung sind lesbar.

**Warum bleibt oder endet der Zustand?** Wiederholtes lokales Cancel beweist weder Prozessende noch bezahlten Endbetrag.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001), [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017)

**Was bleibt warum nutzbar?** Operator kann Anfragen und offene Kosten benennen; keine Behauptung freier Leases oder beendeter Arbeit.

**Zu prüfen:** Unabhängige Runtime-/Providerquittung beweist endgültigen Stop und vollständige Rechnung.

**Architekturfolge für diesen Stressor:** Budgetnull ist keine forcierte Prozessbeendigung. Neue Annahme, bestehende Providerverpflichtung, Runtimebeobachtung und Lease getrennt anzeigen.

<a id="s147"></a>
## S147 — Geschäftskunde verlangt garantierte Echtzeitantwort bei lokalem Single-Host

**Ursprung:** governance; A6-Zustände: GV37, GV04, GV24. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s147.b01"></a>
### S147.B01 — Anforderung ohne Fehlerhülle bleibt unbestimmt

**Art:** offen. **Residue-Status:** teilweise.

**Voraussetzungen:** Unklar sind erlaubter Hostausfall, maximale Rechen-/Providerlatenz und Bedeutung von Echtzeit; vorhandene enge Testbelege bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Eine Forderung allein erzeugt keinen ausführbaren Zeitvertrag.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Prüfer kann bereits belegte begrenzte Funktion von unbewiesener Garantie unterscheiden; geforderte Echtzeit ist nicht erhalten.

**Zu prüfen:** Ein präziser akzeptierter Vertrag samt nachgewiesener Worst-case-Hülle liegt vor.

<a id="s147.b02"></a>
### S147.B02 — Hostausfall zerstört harte Antwortfrist

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Garantie schließt Hostausfall ein; einziger Host ist während unverlängerbarer Antwortfrist unerreichbar und kein erlaubter Ersatz existiert.

**Warum bleibt oder endet der Zustand?** Spätere Wiederkehr kann ursprüngliche Frist nicht erfüllen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für diese rechtzeitige Antwort bleibt keine Restfähigkeit; dauerhafte Datenhaltung ist eine andere Eigenschaft.

**Zu prüfen:** Eine im tatsächlichen Vertrag erlaubte unabhängige Antwortinstanz liefert rechtzeitig.

**Architekturfolge für diesen Stressor:** Harte Echtzeitanforderung erst mit Ausfallhülle und Worst-case-Latenz annehmen. Single-host-Funktion und bewiesene Garantie sind verschiedene Ansprüche.

<a id="s148"></a>
## S148 — Ein Fachauftrag ist nach Queuewartezeit wirtschaftlich wertlos

**Ursprung:** governance; A6-Zustände: GV24, GV21. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s148.b01"></a>
### S148.B01 — Ursprüngliche Gelegenheit verloren

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Harte wirtschaftliche Frist ist abgelaufen, Kundenzweck danach wertlos.

**Warum bleibt oder endet der Zustand?** Vergangene Gelegenheit ist nicht durch schnellere spätere Verarbeitung erneuerbar.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für ursprünglichen ökonomischen Zweck bleibt kein Rest; sinnvolle künftige Verwendung wäre ein anderer Auftrag.

**Zu prüfen:** Kunde bestätigt unveränderten Wert auch nach der Frist.

<a id="s148.b02"></a>
### S148.B02 — Spätes Ergebnis für anderen Zweck brauchbar

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Ergebnisbytes bleiben vorhanden; berechtigter Empfänger nimmt sie ausdrücklich für einen anderen begrenzten Zweck ab.

**Warum bleibt oder endet der Zustand?** Neue Abnahme beendet nur neue Verwendung; alter Fristverlust bleibt verzeichnet.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002)

**Was bleibt warum nutzbar?** Empfänger nutzt echten Folgeauftrag, ohne alten verlorenen Wert als gerettet zu zählen.

**Zu prüfen:** Neue Nutzung hängt weiterhin an der bereits verpassten ursprünglichen Gelegenheit.

**Architekturfolge für diesen Stressor:** Wirtschaftliche Gültigkeit in Warteschlangenauftrag führen und nach Queuewartezeit neu bewerten; technische Fertigstellung darf verpassten Nutzen nicht umetikettieren.

<a id="s149"></a>
## S149 — Unternehmen kann keinen externen Backupdienst mehr bezahlen

**Ursprung:** governance; A6-Zustände: GV30, GV21, GV31. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s149.b01"></a>
### S149.B01 — Primärbetrieb bleibt ohne alten Backupdienst

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Primärbestand funktioniert; tatsächlich vorhandene Ergebnisse sind für weiterhin gültigen engen Zweck abgenommen und lesbar, externe Backupzahlung entfällt.

**Warum bleibt oder endet der Zustand?** Weniger Diversität ist andauernde Exposition, nicht bereits eingetretener Datenverlust.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001), [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002)

**Was bleibt warum nutzbar?** Berechtigte Nutzer lesen alte Aufgaben und abgenommene Ergebnisse; zusätzliche Wiederherstellungsfähigkeit wird nicht behauptet.

**Zu prüfen:** Primärspeicher benötigt den gekündigten Dienst für Lesen oder Ergebnisgebrauch.

<a id="s149.b02"></a>
### S149.B02 — Kleinerer benannter Recoverysatz bleibt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Bereits vorhandener rechtmäßiger Satz mit Schlüssel/Decoder bleibt ohne unbezahlbaren Anbieter tatsächlich nutzbar.

**Warum bleibt oder endet der Zustand?** Engere Wiederherstellung gilt bis nächster gemeinsamer Verlust; ehemalige Standortvielfalt ist weg.

**Zugeordnete Residues:** [GVR012: Vollständig nutzbarer Wiederherstellungssatz](residues.md#gvr012)

**Was bleibt warum nutzbar?** Wiederhersteller kann den konkret noch vorhandenen Umfang lesen, nicht den alten Diversitätsanspruch.

**Zu prüfen:** Isolierter Test ohne Anbieter kann den benannten Inhalt nicht wiederherstellen.

**Architekturfolge für diesen Stressor:** Bezahlbarkeit, Primärbetrieb und Recoverydiversität getrennt führen. Neue verbleibende Kopie muss tatsächlich unabhängig zugänglich sein, nicht als Wunschbackup zählen.

<a id="s150"></a>
## S150 — Unbekannte Providerrechnung macht Metrikvergleich irreführend

**Ursprung:** governance; A6-Zustände: GV22, GV07, GV06. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s150.b01"></a>
### S150.B01 — Geldvergleich offen, Sachwerte vergleichbar

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Rechnung fehlt; ursprüngliche Nutzung und einige gleich definierte Ergebnisgrößen sind erhalten.

**Warum bleibt oder endet der Zustand?** Spätere Rechnung kann Kostenrangfolge ändern; fehlende Geldwerte sind nicht aus Tokens eindeutig ableitbar.

**Zugeordnete Residues:** [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017), [GVR004: Rohmessungen mit begrenztem Vergleich](residues.md#gvr004)

**Was bleibt warum nutzbar?** Analyst nutzt bekannte Sachgrößen und offenes Obligo, nicht behauptete Gewinnrangfolge.

**Zu prüfen:** Vollständiger Tarif und Nutzungsbezug begründen schon eine eindeutige Kostenreihenfolge.

<a id="s150.b02"></a>
### S150.B02 — Scheinbar billiger Pfad entfernt Gegenmessung

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Unbekannte Kosten werden als günstig behandelt und Auswahl belohnt diesen Pfad sowie reduziert unabhängige Abrechnungskontrolle.

**Warum bleibt oder endet der Zustand?** Falscher Vorteil → mehr Auswahl → günstige interne Berichte → weniger Kontrollkosten → fortgesetzter Vorteil.

**Zugeordnete Residues:** [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017)

**Was bleibt warum nutzbar?** Erhaltene echte Nutzungsbelege können spätere Abrechnung tragen; sie erzwingen noch keine Korrektur der Auswahl.

**Zu prüfen:** Endrechnungen bleiben entscheidungswirksam und falsche Rangfolge wird revidiert.

**Architekturfolge für diesen Stressor:** Monetäre Rangfolge an abgerechneten bzw. bindend begrenzten Kosten ausrichten; unbekannter Rechnungsteil darf keinen Nullpreis und kein modellübergreifendes Siegersignal erzeugen.

<a id="s151"></a>
## S151 — Rust-Update ändert Plattformverhalten trotz unverändertem Code

**Ursprung:** governance; A6-Zustände: GV26, GV28, GV25, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s151.b01"></a>
### S151.B01 — Neue Plattformsemantik hält Writer an

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Relevante Abweichung wird vor Mutation erkannt; alter Datenbestand und kompatibler Leser sind erhalten.

**Warum bleibt oder endet der Zustand?** Halt bis neuer Leser/Writer für konkrete Semantik geprüft ist.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020)

**Was bleibt warum nutzbar?** Maintainer prüft unveränderte Daten mit passendem Leser statt sie mit falschem Binary umzudeuten.

**Zu prüfen:** Inkompatibler Writer verändert Daten schon vor Vertragsprüfung.

<a id="s151.b02"></a>
### S151.B02 — Erhaltenes altes Build bleibt begrenzt nutzbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Alter vollständiger Versionssatz darf auf noch kompatibler Plattform betrieben werden; Wechsel wird zurückgestellt.

**Warum bleibt oder endet der Zustand?** Lokaler Betrieb bleibt möglich bis neue Plattform zwingend wird oder Satz ausfällt.

**Zugeordnete Residues:** [GVR016: Rechtmäßig nutzbarer Lauf- und Bausatz](residues.md#gvr016)

**Was bleibt warum nutzbar?** Maintainer kann konkrete alte Verträge weiter bedienen, nicht beliebige neue Plattformen garantieren.

**Zu prüfen:** Altes Binary hängt ebenfalls am geänderten Plattformverhalten oder notwendige Abhängigkeit fehlt.

**Architekturfolge für diesen Stressor:** Buildidentität samt Rust/Plattform und konkrete Verhaltensverträge führen; gleiches Sourcecommit ist keine Laufzeitäquivalenz.

<a id="s152"></a>
## S152 — Release enthält neue Eventversion aber keinen alten Upcaster

**Ursprung:** governance; A6-Zustände: GV26, GV28. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s152.b01"></a>
### S152.B01 — Benötigte alte Eventsemantik unbekannt

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Neuer Reader braucht nicht vorhandene Übersetzung; Originalevents und ein bekannter Präfixleser bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Replay hält an erster unbekannter Stelle, bis korrekte reine Übersetzung existiert.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020)

**Was bleibt warum nutzbar?** Maintainer kann originale Bytes und bekannten Präfix lesen; vollständiger Replay ist nicht erhalten.

**Zu prüfen:** Reader überspringt unbekanntes Event oder ruft beim Lesen Plugins auf.

<a id="s152.b02"></a>
### S152.B02 — Neue Version ist bereits bedeutungsgleich lesbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkrete historische Payloads sind ohne Upcast vollständig kompatibel und semantisch geprüft.

**Warum bleibt oder endet der Zustand?** Releasewechsel braucht für diese Payloads keine Konvertierung; endliche Rekonstruktion kann erfolgen.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020)

**Was bleibt warum nutzbar?** Prüfer nutzt passenden reinen Reader für den tatsächlichen Bestand; fehlender benannter Upcaster ist hier kein Ausfall.

**Zu prüfen:** Ein relevantes altes Feld wird anders interpretiert oder still verworfen.

**Architekturfolge für diesen Stressor:** Eventreplay als Zielvertrag kennzeichnen: unbekannte Version nicht überspringen. Abwärtsverträgliche neue Formate brauchen nicht zwangsläufig einen Upcaster.

<a id="s153"></a>
## S153 — Python-Prototyp und Rust mutieren denselben Tabellennamen mit anderem Schema

**Ursprung:** governance; A6-Zustände: GV26, GV27, GV28. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s153.b01"></a>
### S153.B01 — Ein inkompatibler Writer verweigert Start

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Schema-/Writervertrag erkennt Nichtkompatibilität vor Schreiben; Originaldaten und kompatibler Leser bestehen.

**Warum bleibt oder endet der Zustand?** Halt bis genehmigter Cutover auf genau einen kompatiblen Writer erfolgt.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020)

**Was bleibt warum nutzbar?** Maintainer liest unveränderten Bestand ohne semantisch fremde Mutation.

**Zu prüfen:** Einer der Writer mutiert bereits bevor Kompatibilitätscheck greift.

<a id="s153.b02"></a>
### S153.B02 — Beide Writer verändern akzeptiert dieselbe DB

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Beide inkompatiblen Mutationspfade haben Schreibrechte; relevanter unabhängiger Vorzustandsbeleg ist erhalten.

**Warum bleibt oder endet der Zustand?** Konkurrierende Prozesse halten Deutungskonflikt aufrecht; eine endliche Race wäre nur vorübergehend.

**Zugeordnete Residues:** [GVR033: Unabhängig verwahrter Tatsachenbeleg](residues.md#gvr033)

**Was bleibt warum nutzbar?** Auditor kann nur konkret unabhängig belegte frühere Fakten prüfen; gegenwärtige kohärente Reihenfolge ist nicht bewiesen.

**Zu prüfen:** Eine wirkliche gemeinsame Semantik-/Writergrenze verhindert die zweite Mutation.

**Architekturfolge für diesen Stressor:** Prototyp/Rust-Cutover mit Semantik- und Writergrenze planen; SQL-Transaktion serialisiert konkurrierende Bytes, nicht zwei widersprechende Schemasichten.

<a id="s154"></a>
## S154 — Rollback des Binaries trifft auf bereits migriertes Schema

**Ursprung:** governance; A6-Zustände: GV26, GV28, GV43. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s154.b01"></a>
### S154.B01 — Altes Binary bleibt lesend oder verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Rollbackbinary ist nicht schreibkompatibel; vorgeschaltete Prüfung hält Mutation an, kompatibler Leser für jetzige Bytes bleibt.

**Warum bleibt oder endet der Zustand?** Halt bis verträglicher Writer oder geprüfte bedeutungserhaltende Migration besteht.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020)

**Was bleibt warum nutzbar?** Maintainer bewahrt aktuelle Bytes statt sie durch alte Schemaannahmen zu verändern.

**Zu prüfen:** Historisches Binary schreibt unbemerkt falsche Interpretation.

<a id="s154.b02"></a>
### S154.B02 — DBrollback trifft aktuelle Teilnehmerhistorie

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alter Snapshot wird benutzt, aber aktueller Widerruf und Teilnehmeroperationsregister liegen außerhalb dieses Restores.

**Warum bleibt oder endet der Zustand?** Neue Effekte warten bis Rechte und tatsächliche alte Outcomes getrennt geklärt sind.

**Zugeordnete Residues:** [GVR011: Nicht zurückgesetzter Widerrufsstand](residues.md#gvr011), [GVR021: Restoreunabhängiges Operations- und Ergebnisregister](residues.md#gvr021)

**Was bleibt warum nutzbar?** Gate/Teilnehmer lehnen alte Grants oder bereits akzeptierte Operationen ab; DBintern verlorenes Suffix wird dadurch nicht vollständig rekonstruiert.

**Zu prüfen:** Teilnehmer erkennt alte Identität nicht oder Widerrufsquelle wurde ebenfalls zurückgesetzt.

**Architekturfolge für diesen Stressor:** Rollbackfähigkeit pro exactem Binary und Datenversion prüfen; Datenrestore ist keine Rücknahme nach Snapshot ausgeführter Außenaktionen.

<a id="s155"></a>
## S155 — Ein Versionspaket ist nach Maintainerwechsel nicht mehr herunterladbar

**Ursprung:** governance; A6-Zustände: GV25, GV01, GV04. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s155.b01"></a>
### S155.B01 — Erhaltener Satz unabhängig startbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Authentischer vollständiger Versionssatz samt Rechten und kompatibler Ersatzplattform existiert bereits.

**Warum bleibt oder endet der Zustand?** Downloadausfall beeinträchtigt diesen Satz nicht, bis neue Abhängigkeit benötigt wird.

**Zugeordnete Residues:** [GVR016: Rechtmäßig nutzbarer Lauf- und Bausatz](residues.md#gvr016)

**Was bleibt warum nutzbar?** Maintainer baut/startet die konkret erhaltene Version ohne verschwundenes Paketrepository.

**Zu prüfen:** Offlinebau benötigt noch ein nicht aufbewahrtes Paket oder Recht.

<a id="s155.b02"></a>
### S155.B02 — Nur Bestandsbetrieb, Ersatzinstallation offen

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Bestehender Prozess/Binary funktioniert, aber keine vollständige Ersatzinstallation ist verfügbar; Akte bleibt lesbar.

**Warum bleibt oder endet der Zustand?** Maintainer-/Downloadabsenz hält Erneuerungsblockade aufrecht.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Operator kann vorhandene Aufgaben inspizieren; Neubau- oder Ersatzhardwarefähigkeit wird nicht hinzuerfunden.

**Zu prüfen:** Ersatzumgebung lässt sich bereits aus legal vorhandenen Teilen vollständig starten.

**Architekturfolge für diesen Stressor:** Versionssatz einschließlich Abhängigkeiten und Rechte lokal benennen. Unverfügbare Downloadadresse allein ist weder Laufzeitausfall noch vollständige Reproduzierbarkeit.

<a id="s156"></a>
## S156 — Veröffentlichung exportiert versehentlich Company-root statt Projekt

**Ursprung:** governance; A6-Zustände: GV02, GV08, GV19. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s156.b01"></a>
### S156.B01 — Rootexport vor Sendung als falsch erkannt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Falsches lokales Rootpaket liegt vor, korrektes Projektpaket ist noch nicht erzeugt. Ein konkreter Publikationsauftrag und sein Ablehnungsgrund sind rechtmäßig dauerhaft erhalten. Die Prüfung erkennt Companyinhalte und alle betroffenen noch nicht angenommenen Publikationspfade bleiben tatsächlich gesperrt.

**Warum bleibt oder endet der Zustand?** Freigabe bleibt gesperrt bis korrekt projizierte identische Bytes geprüft sind.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Nutzbar ist nur die erhaltene gesperrte Publikationsabsicht mit dem konkreten Prüfbedarf. Ein sauberes Projektpaket existiert in diesem Zweig noch nicht und wird nicht als Residue gezählt.

**Zu prüfen:** Nur falsches Rootpaket und gehaltenen Auftrag bereitstellen: Inspektion und Nichtsendung müssen ohne jemals erzeugtes korrektes Paket möglich sein. Fehlt die gespeicherte Hülle oder ist ein Sendepfad ungesperrt, trägt auch GVR005 nicht.

**Nachtrag des Koordinators:** [A7S01](../review-dispositions.md#a7s01); ursprüngliche Abgabe unverändert.

<a id="s156.b02"></a>
### S156.B02 — Companyinhalte tatsächlich veröffentlicht und gelesen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Unberechtigte Empfänger haben im gesendeten Rootobjektgraph vertrauliche Inhalte gelesen.

**Warum bleibt oder endet der Zustand?** Spätere Paketkorrektur oder lokale Löschung nimmt die Kenntnis nicht zurück.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Nie-Veröffentlichung/Nie-Offenlegung dieser Inhalte besteht keine Restfähigkeit; erlaubte Projektveröffentlichung bleibt ein anderer Gegenstand.

**Zu prüfen:** Objektgraph enthielt keine fremden vertraulichen Inhalte oder niemand erhielt sie lesbar.

<a id="s156.b03"></a>
### S156.B03 — Tatsächlich vorhandenes geprüftes Projektpaket bleibt nutzbar

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein separates korrekt auf den erlaubten Projektumfang begrenztes Paket ist tatsächlich erzeugt und geprüft. Seine unveränderten Bytes sind rechtmäßig erhalten und außerhalb der Fehlwahl des Rootpakets eindeutig adressierbar. Eine Sendefreigabe wird daraus nicht abgeleitet.

**Warum bleibt oder endet der Zustand?** Die begrenzte Inhaltsprüfung ist abgeschlossen. Das Paket bleibt für berechtigte Nachprüfung oder spätere gesondert erlaubte Veröffentlichung nutzbar, solange Inhalt Rechte und Abnahmegrenze gelten.

**Zugeordnete Residues:** [GVR036: Begrenzt projiziertes Veröffentlichungspaket](residues.md#gvr036)

**Was bleibt warum nutzbar?** Erst hier existiert der von GVR036 bezeichnete saubere Projektobjektgraph. M07 hält die Bytes, M15 den erlaubten Umfang und M02 eine davon getrennte aktuelle Sendefreigabe.

**Zu prüfen:** Wenn nur das falsche Rootpaket vorhanden ist oder die geprüften Bytes später ausgetauscht werden, fehlt dieses Residue. Ohne gesonderte gültige Freigabe darf das vorhandene Paket nicht versandt werden.

**Nachtrag des Koordinators:** [A7S01](../review-dispositions.md#a7s01); ursprüngliche Abgabe unverändert.

**Architekturfolge für diesen Stressor:** Publikation als vorab geprüften Projektobjektgraph binden, einschließlich Historie. Nach Sendung tatsächliche Leser und Kopien statt bloßer lokaler Löschung bewerten.

<a id="s157"></a>
## S157 — Tests bestätigen absichtlich unsicheren Missing-ID-Fallback

**Ursprung:** governance; A6-Zustände: GV06, GV28, GV09, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s157.b01"></a>
### S157.B01 — Unsicherer grüner Test schafft Vertrauen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Falscher Oracle belohnt Missing-ID-Fallback; grüne Ergebnisse verdrängen unabhängige Identitätsprüfung.

**Warum bleibt oder endet der Zustand?** Fallback → grün → Vertrauen → weniger andere Checks → Fallback bleibt legitimiert.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Ein unverändertes Belegverzeichnis zeigt die enge und falsche Testannahme, sofern Originale erhalten; es stellt keine sichere Identität her.

**Zu prüfen:** Unabhängiger Identitätsvertrag blockiert Verhalten trotz grünem Test.

<a id="s157.b02"></a>
### S157.B02 — Fehlende Identität wird unabhängig abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkrete Freigabe benötigt persönlichen Beleg außerhalb des falschen Oracles; dieser fehlt und Gate bleibt geschlossen.

**Warum bleibt oder endet der Zustand?** Halt endet nur mit echtem passenden Beleg oder Ablehnung, nicht mit weiterer grüner Testausführung.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer nutzt die gehaltene Effekthülle; der fehlende Personenbeleg wird ausdrücklich nicht als erhaltenes Residue gezählt. Keine Person entsteht aus missing ID.

**Zu prüfen:** Gleicher Fallback kann selbst den vermeintlich unabhängigen Personenbeleg ausstellen.

**Architekturfolge für diesen Stressor:** Missing-ID-Vertrag muss unabhängig vom absichtlich falschen Testoracle überprüfbar sein; grüne Tests dürfen keine erfundene persönliche Identität schaffen.

<a id="s158"></a>
## S158 — README und ADR widersprechen dem aktuellen Library-Schema

**Ursprung:** governance; A6-Zustände: GV07, GV26, GV05, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s158.b01"></a>
### S158.B01 — Versionierter Reader klärt tatsächlichen Bestand

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkrete Binary-/Schemafassung ist bekannt und kann mit passendem Leser statt allgemeinem README geprüft werden.

**Warum bleibt oder endet der Zustand?** Enger Dokumentationsirrtum endet nach versionierter Klärung; Ziel-ADR bleibt Plan.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020), [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Maintainer prüft reale Lesefähigkeit und begrenzte Behauptungen, ohne Eventreplay aus mutable Tables zu folgern.

**Zu prüfen:** Versionsbezug fehlt oder Reader gibt die dokumentierte Funktion nur vor.

<a id="s158.b02"></a>
### S158.B02 — Widerspruch wird mit alten Dokumenten wegzitiert

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Teams behandeln stale README als Autorität und kopieren es in weitere Entscheidungen trotz entgegenstehender Quellen.

**Warum bleibt oder endet der Zustand?** Dokumentzitat → autoritative Entscheidung → weitere Dokumentzitate → Ausschluss aktueller Befunde.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Unveränderte belegte Gegenbefunde bleiben prüfbar, sofern nicht mit umgeschrieben; Wirksamkeit ihrer Korrektur ist nicht garantiert.

**Zu prüfen:** Aktueller Source-/Verhaltenstest verändert die Entscheidung tatsächlich.

**Architekturfolge für diesen Stressor:** Dokumentations-/ADR-Ziel und ausgelieferte Versionsfähigkeit getrennt versionieren; Widerspruch im gelesenen README ist enger Befund, kein vollständiger Betriebsnachweis.

<a id="s159"></a>
## S159 — Automatischer Updater aktiviert Plugin vor Manifestverifikation

**Ursprung:** governance; A6-Zustände: GV09, GV08, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s159.b01"></a>
### S159.B01 — Ungeprüft gestartetes Paket bleibt zufällig korrekt

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Vorzeitige Aktivierung ist erfolgt, Bytes sind tatsächlich gutartig und Ergebnis konkret unabhängig abgenommen.

**Warum bleibt oder endet der Zustand?** Dieser endliche Lauf kann gelingen; gefährlicher Updatepfad bleibt unverändert möglich.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002)

**Was bleibt warum nutzbar?** Empfänger nutzt nur das belegte richtige Ergebnis; keine Sicherheitsbescheinigung für Activation-before-check.

**Zu prüfen:** Fachliche Abnahme scheitert oder unbekannter Hook hat zusätzliche Wirkung.

<a id="s159.b02"></a>
### S159.B02 — Ungeprüfte Aktivierung hat Kontrolle verloren

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Früh gestartete Bytes sind schädlich; ausgeführte Zugriffe und unabhängige saubere Bereiche noch nicht bestimmt.

**Warum bleibt oder endet der Zustand?** Manifestprüfung danach belegt weder Schadensgrenze noch sauberen Prozesszustand.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für vertrauenswürdige betroffene Ausführung ist kein Residue nachgewiesen; spätere Signaturprüfung ist keine überlebende Schutzstruktur.

**Zu prüfen:** Unabhängige Ausführungsevidenz und Rechteinventar grenzen saubere Objekte konkret ab.

**Architekturfolge für diesen Stressor:** Prüfreihenfolge vor jeder Codeaktivierung einschließlich Hooks erzwingen. Nachträgliches Manifestversagen kann bereits ausgeführte Wirkungen nicht zurückholen.

<a id="s160"></a>
## S160 — Jedes Architekturproblem wird durch weiteren Kerndienst beantwortet

**Ursprung:** governance; A6-Zustände: GV29, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s160.b01"></a>
### S160.B01 — Neue Dienste erzeugen weitere Dienstforderungen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Integrationsprobleme werden belohnt mit weiteren Kerndiensten beantwortet; Gesamtkopplungskosten bleiben unberücksichtigt.

**Warum bleibt oder endet der Zustand?** Mehr Dienste → mehr Schnittstellenfehler → neuer Kernbedarf → mehr Dienste.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Erhaltene Architekturentscheidungen und Gegenbelege bleiben prüfbar; sie reduzieren Kopplung nicht von selbst.

**Zu prüfen:** Nach Vorfällen sinken Gesamtverantwortung/Kopplung durch belegte Zusammenlegung oder Entfernung.

<a id="s160.b02"></a>
### S160.B02 — Begrenzte Änderung vermindert tatsächliche Kopplung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Neue/angepasste Funktion ersetzt belegbar andere Schnittstellen und der konkrete Vertragstest wird abgenommen.

**Warum bleibt oder endet der Zustand?** Endliche Architekturänderung endet; keine Rückkopplung allein aus Dienstanzahl.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Prüfer kann alte und neue Vertragsgrenzen sowie tatsächlichen Nachweis vergleichen.

**Zu prüfen:** Lokale Einsparung verschiebt ungemessene Integration auf andere Dienste.

**Architekturfolge für diesen Stressor:** Neue Verantwortung gegen vorhandenen Kernelvertrag und Gesamtintegration prüfen; module_hint verlangt keinen neuen Dienst. Begründete Vereinfachung und Wachstumsschleife trennen.

<a id="s161"></a>
## S161 — Firmengründer fällt dauerhaft aus und niemand besitzt Freigabeautorität

**Ursprung:** governance; A6-Zustände: GV14, GV33, GV16, GV28. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s161.b01"></a>
### S161.B01 — Alleinige Autorität dauerhaft fehlt

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Gründer ist dauerhaft ausgefallen, kein anerkannter Nachfolger; Effekte mit neuer Pflichtfreigabe werden vermittelt gehalten.

**Warum bleibt oder endet der Zustand?** Institutionelle Lücke endet nicht durch Retry oder Kenntnis des Passworts.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005), [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Berechtigte Leser können alte Aufgaben sehen und Anforderungen aufbewahren; neue legitime Entscheidung bleibt nicht möglich.

**Zu prüfen:** Eine bereits wirksam delegierte verfügbare Person darf genau diese Effekte erlauben.

<a id="s161.b02"></a>
### S161.B02 — Anerkannte Nachfolge erlaubt künftige Arbeit

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Gültige Zuständigkeitsordnung benennt tatsächlich verfügbare befugte Nachfolge und wird umgesetzt.

**Warum bleibt oder endet der Zustand?** Übergang endet mit aktueller Zuordnung; historische Urheberschaft wird nicht umgeschrieben.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032)

**Was bleibt warum nutzbar?** Legitime Nachfolge entscheidet ihren übertragenen Bereich ohne Gründeridentität zu imitieren.

**Zu prüfen:** Nachfolgeregel ist unanerkannt oder konkrete erforderliche Befugnis nicht übertragen.

**Architekturfolge für diesen Stressor:** Legitime Nachfolge von Credentialbesitz unterscheiden; ohne befugte Person keine Selbstermächtigung. Gehaltene Aufgaben und rein lesende Akten sind begrenzte Restfähigkeiten.

<a id="s162"></a>
## S162 — Zwei Geschäftsführer geben widersprüchliche Anweisungen zum selben Effekt

**Ursprung:** governance; A6-Zustände: GV15, GV27, GV38, GV33. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s162.b01"></a>
### S162.B01 — Konflikt hält dieselbe Wirkung zurück

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Anweisungen betreffen denselben Effekt, kein anerkannter Vorrang; gemeinsamer letzter Aufrufpfad sperrt beide.

**Warum bleibt oder endet der Zustand?** Dauert bis bindende Entscheidung oder Verzicht; weitere widersprechende Anweisungen ändern nichts.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer behalten beide Aussagen am konkreten Auftrag; kein Doppelauftrag wird als Vorrangentscheidung benutzt.

**Zu prüfen:** Ein Geschäftsführer kann den gemeinsamen Aufrufpfad umgehen.

<a id="s162.b02"></a>
### S162.B02 — Anerkannte Vorrangregel entscheidet Zukunft

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorab oder legitim neu anerkannte Ordnung legt für diesen Effekt eindeutige Befugnis fest; ein dazu persönlich gebundener neuer Beschluss ist tatsächlich belegt und erhalten.

**Warum bleibt oder endet der Zustand?** Konflikt endet für zukünftige Zulassung; bereits eingetretene Wirkungen werden damit nicht rückwirkend ungeschehen.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032), [GVR010: Personengebundener Freigabebeleg](residues.md#gvr010)

**Was bleibt warum nutzbar?** Befugte Person kann neuen Beschluss persönlich belegen; Übertragung bewahrt nur den tatsächlich gültigen Bereich.

**Zu prüfen:** Beide behaupteten Befugnisse bleiben rechtlich unaufgelöst oder persönlicher Beleg fehlt.

**Architekturfolge für diesen Stressor:** Widerspruch auf Effekt-ID, Zeitpunkt und Vorrang beziehen; letzte Ankunft und zwei legitime Titel sind keine Konfliktlösung. Akzeptierte historische Wirkungen bleiben separat.

<a id="s163"></a>
## S163 — Unternehmen wird verkauft und bisherige Mitarbeiter verlieren Zugriffsrechte

**Ursprung:** governance; A6-Zustände: GV33, GV43, GV09, GV15. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s163.b01"></a>
### S163.B01 — Neue Befugnisse wirksam, alte Grants abgewiesen

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Anerkannte Übergabe ordnet Rechte neu; unabhängiger aktueller Widerrufsstand bindet alle Zulassungspfade; alte persönliche Zustimmungsbelege sind rechtmäßig erhalten.

**Warum bleibt oder endet der Zustand?** Übergang endet sobald konkrete Rechte wirksam sind; zukünftige Änderung kann neue Prüfung verlangen.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032), [GVR011: Nicht zurückgesetzter Widerrufsstand](residues.md#gvr011), [GVR010: Personengebundener Freigabebeleg](residues.md#gvr010)

**Was bleibt warum nutzbar?** Neue Befugte entscheiden, alte persönliche Belege bleiben historisch lesbar und alte Sessions erlangen keine neuen Rechte.

**Zu prüfen:** Eine alte Delegation oder restaurierte Session akzeptiert weiterhin alten Grant.

<a id="s163.b02"></a>
### S163.B02 — Verkauf vollzogen, technische Rechte veraltet

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Rechtlicher Entzug ist erfolgt, aber aufgerufene Gates kennen nur alten Stand; Taskakten bleiben lesbar.

**Warum bleibt oder endet der Zustand?** Lokale historische Grants beweisen die neue Rechtslage nicht; mögliche Nutzung ist separat zu ermitteln.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Auditor kann frühere Grants mit Standgrenze sehen; legitime heutige Autorisierung bleibt unbelegt.

**Zu prüfen:** Aktueller Widerruf erreicht nachweislich alle tatsächlichen Aufrufpfade.

**Architekturfolge für diesen Stressor:** Verkauf in aktuelle Befugnis-/Widerrufsepochen und historische Autorenschaft trennen; Papierentzug muss jede Session, Delegation und Restorezulassung erreichen.

<a id="s164"></a>
## S164 — Projekt wird ausgegliedert und darf Company-Kontext nicht mitnehmen

**Ursprung:** governance; A6-Zustände: GV34, GV08, GV17, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s164.b01"></a>
### S164.B01 — Eigenständiger erlaubter Auftrag bleibt lösbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Projekt darf konkret enthaltene Quellen nutzen und diese reichen fachlich für den Auftrag ohne Companykontext.

**Warum bleibt oder endet der Zustand?** Trennung beeinträchtigt diese enge Aufgabe nicht; andere Aufgaben können blockiert bleiben.

**Zugeordnete Residues:** [GVR022: Rechtmäßig eigenständiger Projektkontext](residues.md#gvr022)

**Was bleibt warum nutzbar?** Nachfolger verwendet nur übertragbare Aufgabenbasis; rechtlich verbotene Kopie wird nicht zur Schutzstruktur.

**Zu prüfen:** Unabhängiger Bearbeiter benötigt unübertragenes Firmenwissen.

<a id="s164.b02"></a>
### S164.B02 — Notwendiges Firmenwissen darf nicht genutzt werden

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Erforderlicher Inhalt ist nicht übertragbar; kein unabhängiger erlaubter Ersatz ist vorhanden und Verwendung wird gesperrt.

**Warum bleibt oder endet der Zustand?** Rechtliche/inhaltliche Lücke bleibt bis zulässige Spezifikation oder Aufgabeaufgabe.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer behält gehaltenen Auftrag mit benannter Lücke; für die verbotene notwendige Wissensfähigkeit bleibt keines.

**Zu prüfen:** Auftrag ist tatsächlich aus bereits erlaubtem Projektmaterial vollständig lösbar.

**Architekturfolge für diesen Stressor:** Spin-out-Kontext explizit nach erlaubten Abhängigkeiten bauen; Root-to-leaf-Kompilierung darf verbotenen Altroot nicht automatisch übertragen.

<a id="s165"></a>
## S165 — Sicherheitsverantwortlicher und Produktowner streiten über riskanten Resume

**Ursprung:** governance; A6-Zustände: GV15, GV02, GV13, GV16. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s165.b01"></a>
### S165.B01 — Risikokonflikt hält Resume

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Security und Produkt haben keinen anerkannten Vorrang; wirksame Effekthülle verhindert neue Annahme.

**Warum bleibt oder endet der Zustand?** Halt endet mit legitimer Risikozuständigkeit; Outcome- und Leasefragen bleiben separat offen.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer bearbeitet konkreten riskanten Resume, ohne aus blockiertem Status Prozessende zu folgern.

**Zu prüfen:** Ein Produktpfad nimmt Resume trotz strittiger Freigabe an.

<a id="s165.b02"></a>
### S165.B02 — Autorität geklärt, altes Outcome weiter offen

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Anerkannte Person darf entscheiden, aber Teilnehmer kann vorherige Wirkung nicht bestätigen; Sperre bleibt bis gewünschter Abgleich.

**Warum bleibt oder endet der Zustand?** Zuständigkeitslösung liefert keine fehlende Außenweltevidenz.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Befugte Person und erhaltene Hülle erlauben kontrollierte Entscheidung über Unsicherheit, nicht Behauptung sicherer Wiederholung.

**Zu prüfen:** Verlässlicher Teilnehmerbeleg klärt den alten Effekt oder Policy erlaubt bewusst neue Wirkung ohne diesen Nachweis.

**Architekturfolge für diesen Stressor:** Risikovorrang und Nachweis früherer Teilnehmerwirkung als unabhängige Sperrprädikate führen. Freigabeautorität kann unbekanntes Outcome nicht durch Behauptung erzeugen.

<a id="s166"></a>
## S166 — Dienstleister ersetzt komplette Agentflotte mit anderen Instrumenten

**Ursprung:** governance; A6-Zustände: GV35, GV07, GV01, GV06. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s166.b01"></a>
### S166.B01 — Messreihen bleiben getrennt, Ergebnis bleibt nutzbar

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Flotte ersetzt, Instrumentbegriffe nicht kalibriert; einige Ergebnisbytes sind unabhängig fachlich abgenommen.

**Warum bleibt oder endet der Zustand?** Fehlende Messsemantik entsteht nicht durch Mittelung; Resultatnutzung kann trotzdem weitergehen.

**Zugeordnete Residues:** [GVR002: Abgenommenes Arbeitsergebnis](residues.md#gvr002), [GVR004: Rohmessungen mit begrenztem Vergleich](residues.md#gvr004)

**Was bleibt warum nutzbar?** Empfänger nutzt Resultate, Analyst nur ursprüngliche enge Rohwerte; keine kontinuierliche Reibungskurve erfunden.

**Zu prüfen:** Die Ergebnisabnahme hängt selbst an inkompatibler alter Metrik.

<a id="s166.b02"></a>
### S166.B02 — Belegter Messschnitt über Instrumentwechsel

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkrete Alt-/Neuversionen wurden für ausdrücklich gleiche Größen repräsentativ kalibriert.

**Warum bleibt oder endet der Zustand?** Nur dieser Schnitt überbrückt Wechsel; ungleiche Felder bleiben getrennt.

**Zugeordnete Residues:** [GVR023: Begrenzte Instrumentkalibrierung](residues.md#gvr023)

**Was bleibt warum nutzbar?** Analyst nutzt belegte Zuordnung ohne fehlendes Feld als null zu werten.

**Zu prüfen:** Gleicher Ablauf liefert für die verglichene Größe systematisch andere Bedeutung.

**Architekturfolge für diesen Stressor:** Alt-/Neuinstrumente versioniert und nur kalibrierte Schnitte vergleichen; Outputabnahme unabhängig von blocked-/turns-Messung halten.

<a id="s167"></a>
## S167 — Teams optimieren auf möglichst wenige blocked Runs und umgehen Freigaben

**Ursprung:** governance; A6-Zustände: GV16, GV06. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s167.b01"></a>
### S167.B01 — Belohnter Bypass stabilisiert sich

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Teams umgehen wirksame Freigabewege tatsächlich und niedrige Blockzahlen bringen Belohnung bei unsichtbaren Verstößen.

**Warum bleibt oder endet der Zustand?** Bypass → niedrige Kennzahl → Belohnung → Norm → mehr Bypass; unterdrückte Verstöße erhalten den Kreis.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Unveränderte konkrete Aufgabenakten erlauben nachträgliche Untersuchung, falls erreichbar; Gateintegrität ist nicht erhalten.

**Zu prüfen:** Unabhängige Sanktionen/Belege ändern Verhalten bei gleicher Last oder kein unzulässiger Effekt liegt vor.

<a id="s167.b02"></a>
### S167.B02 — Weniger Blocks durch legitime Vereinfachung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Zuständige Stelle entfernt unnötige Schritte, persönliche Zustimmungen und verbotene Effekte werden unabhängig geprüft.

**Warum bleibt oder endet der Zustand?** Begrenzte Verbesserung reduziert Kosten ohne verdeckte neue Befugnis; kein Bypassloop erforderlich.

**Zugeordnete Residues:** [GVR010: Personengebundener Freigabebeleg](residues.md#gvr010), [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Auditor nutzt Effektbelege und explizit geänderten Anspruch, statt niedrige Kennzahl als alleinigen Erfolg zu nehmen.

**Zu prüfen:** Neue unautorisierte Wirkungen werden unter geändertem Nenner verborgen.

**Architekturfolge für diesen Stressor:** Blockedzahl nicht als alleinigen Sicherheitserfolg führen; genehmigte Vereinfachung und tatsächlichen Bypass anhand unabhängiger Effekt-/Zustimmungsbelege unterscheiden.

<a id="s168"></a>
## S168 — Scope-Besitzer verweigert Budgetauskunft obwohl gemeinsame Maschine überlastet ist

**Ursprung:** governance; A6-Zustände: GV10, GV22, GV23. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s168.b01"></a>
### S168.B01 — Verbrauchsanteile funktionieren ohne Budgetoffenlegung

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Vertraulichkeit des Budgets bleibt; unabhängige zulässige Ressourcenrechnung und verbindliche Anteile sind durchgesetzt.

**Warum bleibt oder endet der Zustand?** Überlast hält abgewiesene Nachfrage aufrecht, aber nicht die garantierten Anteile.

**Zugeordnete Residues:** [GVR019: Durchsetzbare Ressourcenanteile](residues.md#gvr019)

**Was bleibt warum nutzbar?** Operator und kleine Scopes nutzen konkreten Zuteilungsvertrag, ohne privaten Budgetbetrag zu kennen.

**Zu prüfen:** Verfügbare Ressourcenmessung genügt für vereinbarte Anteile nicht oder wird manipuliert.

<a id="s168.b02"></a>
### S168.B02 — Undurchsichtige Belegung verstärkt Einfluss

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Belegung schafft politischen Zuteilungsvorteil; Auskunftsverweigerung unterbindet gerade die bindende Gegenrechnung.

**Warum bleibt oder endet der Zustand?** Mehr Belegung → mehr Einfluss → Schutz undurchsichtiger Zuteilung → mehr Belegung.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Erhaltene Akten können Streit und bekannte Last dokumentieren; faire Zuteilung ist daraus nicht automatisch rekonstruierbar.

**Zu prüfen:** Akzeptierte unabhängige Verbrauchsrechnung löst Verteilung ohne Budgetpreisgabe oder Einfluss hängt nicht an Belegung.

**Architekturfolge für diesen Stressor:** Ressourcenverbrauch und faire Zuteilung ohne Offenlegung fremder Geschäftsbudgets erfassen, sofern zulässig. Budgetgeheimhaltung allein ist weder Missbrauch noch Kapazitätsmessung.

<a id="s169"></a>
## S169 — Organisation verlangt nachträgliches Umschreiben eines unbequemen Auditfakts

**Ursprung:** governance; A6-Zustände: GV15, GV28, GV16, GV05. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s169.b01"></a>
### S169.B01 — Ursprünglicher Fakt bleibt unabhängig überprüfbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Organisation verlangt Rewrite, aber konkreter Originalfakt ist außerhalb der angreifbaren Schreibgrenze legal verwahrt.

**Warum bleibt oder endet der Zustand?** Lokaler Änderungswunsch beseitigt diesen Beleg nicht; Streit über neue Bewertung kann bleiben.

**Zugeordnete Residues:** [GVR033: Unabhängig verwahrter Tatsachenbeleg](residues.md#gvr033)

**Was bleibt warum nutzbar?** Auditor kann Original und spätere sichtbare Korrektur vergleichen, nicht jede denkbare Aussage authentifizieren.

**Zu prüfen:** Verwahrer ist derselben Änderungshoheit unterworfen oder hat strittigen Fakt nie erhalten.

<a id="s169.b02"></a>
### S169.B02 — Günstige Umschreibung legitimiert weitere

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Originale werden ohne verbleibenden unabhängigen Vergleich umgeschrieben; bessere Auditgeschichte bringt Nutzen und macht weiteres Umschreiben akzeptabel.

**Warum bleibt oder endet der Zustand?** Rewrite → günstige Geschichte → Belohnung/Legitimation → weiterer Rewrite.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für unverfälschten betroffenen historischen Fakt bleibt ohne unabhängige Darstellung kein nutzbarer Beleg; übriger Betrieb kann laufen.

**Zu prüfen:** Originale bleiben unabhängig nachweisbar und sichtbare Sanktion beendet Umschreibungsnorm.

**Architekturfolge für diesen Stressor:** Sichtbare Korrektur mit ursprünglichem Fakt und Autor von verdecktem Rewrite trennen. Append-only Ziel braucht tatsächliche Schreibgrenze; gegen Root ist unabhängige Verwahrung nötig.

<a id="s170"></a>
## S170 — Ein Partner stellt Betrieb ein und niemand kann seine alten Operationen bestätigen

**Ursprung:** governance; A6-Zustände: GV03, GV13, GV01, GV38. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s170.b01"></a>
### S170.B01 — Kein Zeuge für alte Annahme, Wiederholung gehalten

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Partner ist weg, kein unabhängiger Outcome bekannt; alte Versuchsakte und wirksame neue Effektsperre bestehen.

**Warum bleibt oder endet der Zustand?** Gleiche lokale Abfrage erzeugt kein Außenweltergebnis; bewusste neue Entscheidung kann Risiko ändern, nicht Vergangenheit.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer sieht offenen Versuch und kann neue Wirkung zurückhalten; kein Beweis gescheiterten ersten Effekts.

**Zu prüfen:** Bereits vorhandener unabhängiger Receipt klärt die Annahme oder Pfad sendet trotzdem erneut.

<a id="s170.b02"></a>
### S170.B02 — Vorher gesicherter konkreter Receipt klärt Wirkung

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Ergebnisbeleg aus Teilnehmerregister war vor Stilllegung unabhängig lesbar erhalten und bindet genau diese Operations-ID.

**Warum bleibt oder endet der Zustand?** Konkrete Outcome-Frage endet mit belegtem Ergebnis; übrige Partneroperationen bleiben offen.

**Zugeordnete Residues:** [GVR021: Restoreunabhängiges Operations- und Ergebnisregister](residues.md#gvr021)

**Was bleibt warum nutzbar?** Reconciler nutzt den erhaltenen Receipt, nicht eine erfundene künftig erreichbare Partnerantwort; laufendes Fencing ist damit nicht garantiert.

**Zu prüfen:** Receipt fehlt für fragliche Operation oder beweist nur lokale Sendeabsicht.

**Architekturfolge für diesen Stressor:** Partnerstilllegung lässt Outcome unbekannt statt fehlgeschlagen werden. Lokale Versuche, unabhängige tatsächliche Teilnehmerbelege und neue Effektautorität separat halten.

<a id="s171"></a>
## S171 — Tausend neue Scopes erzeugen je einen Pluginprozess

**Ursprung:** governance; A6-Zustände: GV10, GV04, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s171.b01"></a>
### S171.B01 — Prozessanzahl trifft wirksame Gesamtreserve

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Aktivierung misst und begrenzt aggregierten Footprint; Betriebsreserve und konkrete Anteile sind real durchgesetzt.

**Warum bleibt oder endet der Zustand?** Mehr Scopes werden zurückgestellt statt Reserve zu verbrauchen; Dauerlast bleibt externe Nachfrage.

**Zugeordnete Residues:** [GVR019: Durchsetzbare Ressourcenanteile](residues.md#gvr019), [GVR027: Unabhängige Startdiagnose](residues.md#gvr027)

**Was bleibt warum nutzbar?** Operator behält lokalen Inspektionspfad und zugelassene Scopes ihren Anteil.

**Zu prüfen:** Startspitze oder ungemessener Nebenprozess verdrängt trotz Limit den lokalen Zugang.

<a id="s171.b02"></a>
### S171.B02 — Unbegrenzte Residentlast verdrängt Pflichtdienst

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Jeder Scope startet Prozess ohne Gesamtlimit, Footprint überschreitet Hostkapazität; intakte Akte bleibt später lesbar.

**Warum bleibt oder endet der Zustand?** Last hält Ausfall bis Prozesse enden oder Ressourcen frei werden; Neustartloop braucht zusätzliche Policy.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Operator kann nach verfügbarem Leser bestehende Aufgaben untersuchen; keine verfügbare Daemonausführung während Verdrängung behauptet.

**Zu prüfen:** Tatsächlicher Footprint passt in ausreichende Reserve und Pflichtdienst bleibt erreichbar.

**Architekturfolge für diesen Stressor:** Pro-Scope-Aktivierung bleibt explizit, benötigt aber aggregierte Prozess-/Speicherzulassung. Tausend Prozesse sind ohne Footprint kein Beweis notwendiger Instabilität.

<a id="s172"></a>
## S172 — Zwei Kunden fordern gegenseitige Geheimhaltung auf derselben User-ID

**Ursprung:** governance; A6-Zustände: GV37, GV08, GV20. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s172.b01"></a>
### S172.B01 — Zusätzliche nachgewiesene Grenze trennt Kundendaten

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Trotz gleicher UID erzwingt unabhängige Sandbox-/Keygrenze tatsächlich gegenseitige Nichtlesbarkeit für alle relevanten Pfade.

**Warum bleibt oder endet der Zustand?** Koexistenz kann unter genau diesem Bedrohungsvertrag andauern.

**Zugeordnete Residues:** [GVR008: Außerhalb des Angreiferzugriffs liegender Datenbereich](residues.md#gvr008)

**Was bleibt warum nutzbar?** Jeweiliger berechtigter Kunde nutzt eigenen Bestand ohne Zugriff des anderen; Scope-Namen allein erfüllen Voraussetzung nicht.

**Zu prüfen:** Ein erlaubter Kundencodepfad liest über gemeinsame Dateien oder Keychain den fremden Inhalt.

<a id="s172.b02"></a>
### S172.B02 — Nur gemeinsame UID, geforderte Isolation unbelegt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Konkrete Sandbox, Schlüsselrechte und native Zugriffspfade sind nicht spezifiziert; kein tatsächliches Lesen nachgewiesen.

**Warum bleibt oder endet der Zustand?** Fehlender Grenznachweis wird nicht durch zwei Scopeeinträge ersetzt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für zugesagte gegenseitige Geheimhaltung ist kein Residue belegt; Offenlegung darf daraus dennoch nicht behauptet werden.

**Zu prüfen:** Rechteprüfung belegt Nichtlesbarkeit oder zeigt einen tatsächlichen fremden Lesezugriff.

**Architekturfolge für diesen Stressor:** Kundengeheimhaltung nach effektivem Angreifer-/UID-Rechtevertrag bewerten. Gemeinsame User-ID allein beweist weder Isolation noch tatsächlichen Leak.

<a id="s173"></a>
## S173 — Remote-Worker laufen über Tage offline auf fremder Hardware

**Ursprung:** governance; A6-Zustände: GV36, GV43, GV27, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s173.b01"></a>
### S173.B01 — Offline nur haltbare Entwürfe

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Worker darf lokal auf vertrauenswürdig begrenzter Fremdhardware arbeiten, Absichten bleiben ohne Außenwirkung; Paket ist haltbar.

**Warum bleibt oder endet der Zustand?** Offlinezeit verhindert frische Autorität, nicht diese selbständige Entwurfsarbeit; Import verlangt neuen Abgleich.

**Zugeordnete Residues:** [GVR024: Offline-Arbeitspaket ohne heutige Vollmacht](residues.md#gvr024)

**Was bleibt warum nutzbar?** Berechtigter Worker nutzt gespeicherte Entwürfe und Basisversionen; keine Zusage heutiger externer Gültigkeit.

**Zu prüfen:** Fremder Host liest unerlaubte Daten oder Worker setzt veränderliche Außenvollmacht offline ein.

<a id="s173.b02"></a>
### S173.B02 — Alte Absicht trifft neue Rechte und Teilnehmerstände

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Bei Rückkehr greifen unabhängiger Widerrufsstand und Teilnehmerabgleich vor Annahme des Entwurfs als Effekt.

**Warum bleibt oder endet der Zustand?** Veraltete Absicht bleibt bis gültiger Neubewertung gehalten; erfolgreiche Dateiübertragung beendet das nicht.

**Zugeordnete Residues:** [GVR011: Nicht zurückgesetzter Widerrufsstand](residues.md#gvr011), [GVR021: Restoreunabhängiges Operations- und Ergebnisregister](residues.md#gvr021)

**Was bleibt warum nutzbar?** Reconciler kann alte Grants/Operationsidentitäten prüfen und Doppelwirkung verweigern, sofern Teilnehmer dies vertraglich unterstützt.

**Zu prüfen:** Offlinewirkung wurde schon ohne diese Grenze angenommen oder Teilnehmer kennt Operations-ID nicht.

**Architekturfolge für diesen Stressor:** Offlineentwurf von aktueller Mutationsbefugnis und Fremdhardwarevertrauen trennen. Reconnect braucht aktuelle Rechte und Objektversionen, nicht bloßen Uploadabschluss.

<a id="s174"></a>
## S174 — Produkt muss auf Smartphone ohne Daemon-Dauerbetrieb laufen

**Ursprung:** governance; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s174.b01"></a>
### S174.B01 — Erlaubter Remoteclient statt lokaler Dauerdaemon

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Vertrag erlaubt benannten tatsächlich verfügbaren Remote-Kern und verlangt lokale Nutzung nur während Verbindung.

**Warum bleibt oder endet der Zustand?** OS-Suspension unterbricht Zugang, nicht automatisch entfernte Ausführung; Beobachterverlust ist getrennt.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Berechtigter Client kann beim Wiederverbinden den haltbaren bekannten Bestand lesen; autonome Offlineantwort ist nicht erhalten.

**Zu prüfen:** Vertrag verlangt volle autonome lokale Funktion oder Remote-Kern ist nicht tatsächlich verfügbar.

<a id="s174.b02"></a>
### S174.B02 — Intermittente lokale Entwürfe überstehen Suspension

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Nur selbständige Entwürfe gefordert; lokales haltbares Arbeitspaket passt in erlaubte OS-Laufzeiten und Rechte.

**Warum bleibt oder endet der Zustand?** Suspension pausiert Arbeit, nächster erlaubter Start kann Entwurf lesen; frische Außenvollmacht bleibt aus.

**Zugeordnete Residues:** [GVR024: Offline-Arbeitspaket ohne heutige Vollmacht](residues.md#gvr024), [GVR028: Wiederaufnehmbarer Arbeitsstand](residues.md#gvr028)

**Was bleibt warum nutzbar?** Worker/Benutzer kann gespeicherte Teilaufgabe wiederaufnehmen ohne dauernden Daemon.

**Zu prüfen:** OS beendet vor jedem nötigen Commit oder Auftrag erfordert während Suspension aktuelle Außenwirkung.

<a id="s174.b03"></a>
### S174.B03 — Autonome harte Hintergrundwirkung ohne Laufrecht

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Vertrag verlangt Fristwirkung während vollständiger OS-Suspension, kein erlaubter Remoteexecutor oder OS-Ausführungsfenster existiert.

**Warum bleibt oder endet der Zustand?** Technischer Halt dauert mit Suspension; Fristverpassung kann folgen, nicht interne Konvergenz.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für genau diese autonome Fristwirkung besteht kein nutzbarer Ausführungsträger. Andere Smartphonefunktionen bleiben möglich.

**Zu prüfen:** OS gewährt ein ausreichend verlässliches Fristfenster oder Vertrag erlaubt tatsächlich verfügbaren Remoteexecutor.

**Architekturfolge für diesen Stressor:** Smartphonevertrag zuerst festlegen: Remoteclient, intermittente Entwurfsarbeit oder autonome Fristwirkung. Kein versteckter ständig verfügbarer Fremddaemon wird vorausgesetzt.

<a id="s175"></a>
## S175 — Firma verlangt Active-active mit automatischem Failover zwischen Städten

**Ursprung:** governance; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s175.b01"></a>
### S175.B01 — Partition lässt nur berechtigte Seite mutieren

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vertrag erlaubt Unverfügbarkeit einer Stadt; Teilnehmer erzwingt exklusive Epoche und stabile Operations-IDs außerhalb lokaler Kopien.

**Warum bleibt oder endet der Zustand?** Abgetrennte Seite hält neue Wirkung bis gültige Epoche/Abgleich vorliegt, andere kann ihren zulässigen Teil leisten.

**Zugeordnete Residues:** [GVR021: Restoreunabhängiges Operations- und Ergebnisregister](residues.md#gvr021), [GVR038: Unabhängig erzwungene Schreibepoche](residues.md#gvr038)

**Was bleibt warum nutzbar?** Reconciler und Teilnehmer bewahren begrenzte Effektordnung; gleichzeitige volle Verfügbarkeit wird ausdrücklich aufgegeben.

**Zu prüfen:** Beide Städte können bei Partition dieselbe externe Wirkung unter akzeptierter Epoche auslösen.

<a id="s175.b02"></a>
### S175.B02 — Beide Städte schreiben widersprüchlich weiter

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Vertrag/Implementierung lässt ohne gemeinsame Fencinggrenze beide denselben Effektbereich bedienen; getrennte lokale Aufgaben-/Versuchsakten bleiben lesbar erhalten.

**Warum bleibt oder endet der Zustand?** Anhaltende Partition und Doppelbefugnis halten Divergenz; bloße Bytekonsistenz jeder DB löst sie nicht.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Prüfer kann nur tatsächlich erhaltene lokale Absichten als historischen Arbeitsstand vergleichen; gemeinsame Reihenfolge und ein akzeptiertes Outcome sind nicht bewiesen.

**Zu prüfen:** Teilnehmer serialisiert die Wirkung tatsächlich unter einer anerkannten gemeinsamen Identität.

<a id="s175.b03"></a>
### S175.B03 — Garantieziel ohne Partitionsentscheidung

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Unverfügbarkeit, Quorum, Effektbesitz und Latenz wurden nicht entschieden.

**Warum bleibt oder endet der Zustand?** Forderung allein bestimmt keine Konvergenz oder überlebende globale Fähigkeit.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für behauptetes garantiertes active-active gibt es ohne Vertrag keinen Restnachweis; keine automatische Architekturwahl wird erfunden.

**Zu prüfen:** Ein akzeptierter Partitions-/Teilnehmervertrag ordnet konkrete Ausfalltraces eindeutig zu.

**Architekturfolge für diesen Stressor:** Active-active-Vertrag braucht Partitionspolitik und extern durchgesetzte Operations-/Writeridentität. Mehrere DBs allein liefern weder eine Wahrheit noch automatisches sicheres Failover.

<a id="s176"></a>
## S176 — Robotikplugin steuert irreversible Bewegung mit Millisekundenfrist

**Ursprung:** governance; A6-Zustände: GV37, GV20, GV38. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s176.b01"></a>
### S176.B01 — Lokaler Controller hält enge Bewegungsgrenze

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Bereits vorhandener separat validierter Controller/Sensorik beherrscht konkrete Gefahr und Frist unabhängig von Factorylatenz.

**Warum bleibt oder endet der Zustand?** Factoryverzögerung hält neue Aufträge auf; Controller kann erlaubte lokale Sicherheitsreaktion ausführen.

**Zugeordnete Residues:** [GVR035: Begrenzte lokale Bewegungsfreigabe](residues.md#gvr035)

**Was bleibt warum nutzbar?** Anlagenverantwortlicher nutzt den nachgewiesenen lokalen Regel-/Sperrpfad; kein Beleg heutiger Factory-Echtzeitfähigkeit.

**Zu prüfen:** Sensor-/Controllerfehler oder Bewegung außerhalb geprüfter Hülle verletzt die behauptete Grenze.

<a id="s176.b02"></a>
### S176.B02 — Irreversible Bewegung bereits erfolgt

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Bewegung wurde physisch angenommen und die betrachtete Vorbewegungseigenschaft ist nicht exakt wiederherstellbar.

**Warum bleibt oder endet der Zustand?** Historisches Eintreten lässt sich nicht löschen; legitime harmlose Bewegung bleibt ebenfalls erfolgt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Ungeschehenmachen dieser Eigenschaft bleibt nichts; das bedeutet weder automatisch Schaden noch Verlust aller Anlagenfähigkeit.

**Zu prüfen:** Bewegung fand nicht statt oder eine tatsächlich exakte Inverse erhält alle betrachteten Eigenschaften.

**Architekturfolge für diesen Stressor:** Factory nicht in Millisekundenregelkreis verlegen. Separater validierter Controller kann eine präzise physische Grenze tragen, nicht universelle Unumkehrbarkeit beseitigen.

<a id="s177"></a>
## S177 — Regulierter Betrieb verlangt manipulationssichere Beweise gegen Root-Admin

**Ursprung:** governance; A6-Zustände: GV37, GV28, GV01. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s177.b01"></a>
### S177.B01 — Konkreter außerhalb Root verwahrter Fakt prüfbar

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Benannter historischer Fakt wurde vorher bei rechtlich erlaubtem unabhängig kontrolliertem Verwahrer belegt; Root erreicht ihn nicht.

**Warum bleibt oder endet der Zustand?** Lokale Manipulation zerstört den Vergleichsbeleg nicht, unverwahrte spätere Fakten bleiben unbewiesen.

**Zugeordnete Residues:** [GVR033: Unabhängig verwahrter Tatsachenbeleg](residues.md#gvr033)

**Was bleibt warum nutzbar?** Autorisierter Auditor prüft den konkret erhaltenen Fakt und erkannte Abweichung.

**Zu prüfen:** Root kontrolliert auch Belegquelle/Schlüssel oder strittiger Fakt liegt außerhalb ihres Inhalts.

<a id="s177.b02"></a>
### S177.B02 — Nur Rootkontrollierte Beweise vorhanden

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Alle verfügbaren Belegbytes, Code und Trustanker liegen unter dem angreifenden Root; kein unabhängiger Herkunftsbeleg.

**Warum bleibt oder endet der Zustand?** Neue lokale Hashes oder Signaturen reparieren die fehlende unabhängige Vergangenheit nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für verlangten Rootfestigkeitsbeweis dieser Historie besteht kein Residue; lesbare Bytes können trotzdem weiter nützlich sein.

**Zu prüfen:** Ein tatsächlich unabhängiger zeitgenössischer Verwahrbeleg wird nachgewiesen.

**Architekturfolge für diesen Stressor:** Gegen Root benötigter Beweis braucht benannte unabhängige Verwahrung jenseits lokaler Dateien und Code. Lokales Append-only ist kein manipulationssicherer Menschenbeweis.

<a id="s178"></a>
## S178 — Ereignisbestand wächst auf Jahrzehnte und Replay dauert länger als RTO

**Ursprung:** governance; A6-Zustände: GV47, GV24, GV26. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s178.b01"></a>
### S178.B01 — Korrekte endliche Wiederherstellung kommt zu spät

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Alle benötigten Inhalte und Leser sind nutzbar, gemessene vollständige Dauer überschreitet RTO.

**Warum bleibt oder endet der Zustand?** Endlicher Replay kann später fertig werden; vergangene Zeitgrenze bleibt verfehlt.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020), [GVR025: Begrenzter Wiederherstellungsnachweis](residues.md#gvr025)

**Was bleibt warum nutzbar?** Maintainer rekonstruiert Inhalt und Auditor sieht ehrlichen Laufzeitbeleg; RTO-Fähigkeit ist nicht erhalten.

**Zu prüfen:** Repräsentative vollständige Messung einschließlich Inhalt/Tail liegt innerhalb RTO.

<a id="s178.b02"></a>
### S178.B02 — Validierter schneller Pfad erfüllt enges RTO

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkreter gültiger Snapshot samt vollständigem Tail/Content und reinem Reader wurde auf relevanter Ersatzhardware innerhalb RTO abgenommen.

**Warum bleibt oder endet der Zustand?** Dieser Restore endet; Wachstum oder anderer Schaden braucht neuen Nachweis.

**Zugeordnete Residues:** [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020), [GVR025: Begrenzter Wiederherstellungsnachweis](residues.md#gvr025)

**Was bleibt warum nutzbar?** Wiederhersteller nutzt belegten Interpretationspfad und Auditor die zeitbegrenzte Evidenz, keine bloße Snapshotbehauptung.

**Zu prüfen:** Snapshotvalidierung oder Tailbeschaffung außerhalb Messung überschreitet tatsächliche Frist.

**Architekturfolge für diesen Stressor:** RTO mit vollständig validiertem Reader, Inhalt, Snapshot und Tail messen. Langsamer korrekter Replay ist kein Informationsverlust; Snapshotexistenz beweist keine Beschleunigung.

<a id="s179"></a>
## S179 — Kunde fordert vollständigen Export und Abschaltung aller Factorydienste

**Ursprung:** governance; A6-Zustände: GV46, GV31, GV34, GV19. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s179.b01"></a>
### S179.B01 — Lokale Dienste beendet, Übergabe verwendbar

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Empfänger hat erlaubten vollständigen Export ohne Factory gelesen; alle für lokale Abschaltung geforderten Factorydienste sind tatsächlich aus.

**Warum bleibt oder endet der Zustand?** Lokaler Lebenszyklus endet; Teilnehmerrestpflichten und Remote-Kopien werden damit nicht null.

**Zugeordnete Residues:** [GVR026: Ohne Factory lesbare Übergabe](residues.md#gvr026)

**Was bleibt warum nutzbar?** Empfänger nutzt tatsächlich enthaltene Bytes/Bedeutung nach lokalem Stop; keine universelle Stilllegung externer Welt behauptet.

**Zu prüfen:** Empfänger benötigt doch einen Factorydienst oder lokaler Worker arbeitet weiter.

<a id="s179.b02"></a>
### S179.B02 — Lokaler Stop mit offener externer Wirkung

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Factory ist lokal aus, aber frühere akzeptierte Teilnehmeroperation ist noch offen; erreichbarer unabhängiger Operationsbeleg existiert.

**Warum bleibt oder endet der Zustand?** Externer Teilnehmer kann später handeln; Gesamtabschluss braucht Outcome und vereinbarten Beobachtungshorizont.

**Zugeordnete Residues:** [GVR021: Restoreunabhängiges Operations- und Ergebnisregister](residues.md#gvr021)

**Was bleibt warum nutzbar?** Berechtigter Abwickler kann konkrete Operation außerhalb Factory prüfen; Empfängerexport kann gleichzeitig nutzbar sein.

**Zu prüfen:** Alle Teilnehmerpflichten sind nachweislich beendet oder angeblicher Beleg enthält nur lokale Absicht.

<a id="s179.b03"></a>
### S179.B03 — Export ist nur Manifest ohne notwendige Inhalte

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Empfänger erhält nur Struktur/Referenzen und benötigt fehlende Payload oder nicht übertragbaren Kontext.

**Warum bleibt oder endet der Zustand?** Dienstabschaltung erzeugt weder fehlende Bytes noch Rechte/Bedeutung.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für vereinbarte vollständige selbständige Nutzung bleibt keine Restfähigkeit aus diesem Export; lesbare Teilmetadaten sind nicht Vollübergabe.

**Zu prüfen:** Empfänger beantwortet sämtliche vereinbarten konkreten Fragen rechtmäßig ohne Factory und ohne fehlende Quelle.

**Architekturfolge für diesen Stressor:** Exportabnahme, lokale Dienstbeendigung, externe Teilnehmerpflichten und spätere Kopienlöschung getrennt abschließen. Exportcontainer nur unter .factory erzeugen; Empfängernutzung ist eigener Vorgang.

<a id="s180"></a>
## S180 — Pluginlandschaft entwickelt wechselseitige Abhängigkeiten mit Startzyklus

**Ursprung:** governance; A6-Zustände: GV45, GV01, GV10. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s180.b01"></a>
### S180.B01 — Strenger Bereitschaftszyklus, Diagnose bleibt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** A braucht B vollständig ready und B braucht A; kein Seed, aber eigener lokaler Inspektionspfad bleibt unabhängig funktionsfähig.

**Warum bleibt oder endet der Zustand?** Wechselseitiges Warten ist durch Startregeln gehalten, kein bewiesener Attraktor.

**Zugeordnete Residues:** [GVR027: Unabhängige Startdiagnose](residues.md#gvr027)

**Was bleibt warum nutzbar?** Operator liest die konkreten Kanten und kann separat begründete Reparatur planen, ohne Plugins bereits starten zu müssen.

**Zu prüfen:** Diagnose hängt selbst an A/B oder eine bestehende Seedbereitschaft löst den Zyklus.

<a id="s180.b02"></a>
### S180.B02 — Optionale Bindung erlaubt endlichen Start

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Mindestens eine Kante ist tatsächlich lazy/optional; zulässiger Bootstrap und ausreichende Ressourcen existieren.

**Warum bleibt oder endet der Zustand?** Erste echte Bereitschaft löst die übrigen nötigen Voraussetzungen; Zyklus allein verursacht keinen Deadlock.

**Zugeordnete Residues:** [GVR027: Unabhängige Startdiagnose](residues.md#gvr027)

**Was bleibt warum nutzbar?** Operator nutzt Startbeschreibung zur Prüfung der wirklich optionalen Grenze; erfolgreiche Facharbeit ist damit noch nicht belegt.

**Zu prüfen:** Die angeblich optionale Bindung wird zur Bereitschaft doch zwingend benötigt.

**Architekturfolge für diesen Stressor:** Bereitschaftsabhängigkeiten von optionalen Bindungen trennen; unabhängiger lokaler safe-mode Zugang darf nicht Mitglied des Pluginstartzyklus sein.

<a id="s181"></a>
## S181 — Extremer Sonnensturm zerstört Elektronik in mehreren Backupregionen

**Ursprung:** governance; A6-Zustände: GV04, GV30, GV48. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s181.b01"></a>
### S181.B01 — Unbetroffener Satz bleibt konkret wiederherstellbar

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Bereits vorhandenes legal zugängliches Medium samt Schlüssel/Decoder außerhalb zerstörter Elektronik ist mit verfügbarer Ersatztechnik lesbar.

**Warum bleibt oder endet der Zustand?** Rekonstruktion kann enden, verbleibende Standortdiversität ist enger als vorher.

**Zugeordnete Residues:** [GVR012: Vollständig nutzbarer Wiederherstellungssatz](residues.md#gvr012)

**Was bleibt warum nutzbar?** Wiederhersteller nutzt nur tatsächlich überlebenden vollständigen Satz, keine implizit perfekte zusätzliche Region.

**Zu prüfen:** Ersatztechnik oder Schlüssel teilt denselben Schadensbereich und Inhalt kann nicht gelesen werden.

<a id="s181.b02"></a>
### S181.B02 — Alle bekannten praktischen Inhaltswege ausgefallen

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Inventar zeigt keine bekannte nutzbare Kopie/Schlüssel/Decoderkombination; Erinnerungen oder Identifikatoren können bleiben.

**Warum bleibt oder endet der Zustand?** Warten allein liefert keine Elektronik/Inhalte; dauerhafte vollständige Informationsvernichtung ist nicht bewiesen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für derzeitige Inhaltsrecovery besteht kein nutzbarer Rest; mehrere Regionen bedeuten nicht S190.

**Zu prüfen:** Ein tatsächlich unabhängiger Inhaltspfad wird auf verfügbarer Technik demonstriert.

**Architekturfolge für diesen Stressor:** Mehrere zerstörte Regionen nicht mit vollständigem Weltverlust gleichsetzen. Medien, Schlüssel, Decoder und Ersatzenergie im benannten Schadensinventar gemeinsam prüfen.

<a id="s182"></a>
## S182 — Mac erwacht im Jahr 2046 und alle Termine Zertifikate und Anbieter sind historisch

**Ursprung:** governance; A6-Zustände: GV36, GV25, GV04, GV10, GV43. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s182.b01"></a>
### S182.B01 — Historische Akte lesbar, heutige Wirkung gehalten

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Lokaler Bestand hat lesbaren alten Stand; aktuelle Zeit/Autorität/Provider sind unbestätigt und alle neuen Effekte bleiben gesperrt.

**Warum bleibt oder endet der Zustand?** Jahrzehntealter Kalender liefert keine frische Zustimmung; jede fehlende Voraussetzung hat eigenen Ausgang.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer liest historischen Plan und offene Absichten; gegenwärtige Ausführung und Bedeutung sind nicht automatisch verfügbar.

**Zu prüfen:** Aufwachen führt ohne Abgleich alte Termine als aktuelle Wirkungen aus.

<a id="s182.b02"></a>
### S182.B02 — Nachholwelle bleibt endliche neue Prüfung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Aktuelle zuständige Person ist legitim verfügbar; Cronmaterialisierung besitzt begrenzte Wurzeln und überprüft Gültigkeit statt blind nachzuholen.

**Warum bleibt oder endet der Zustand?** Alte Vorkommen werden endlich triagiert; abgelaufene Fristen bleiben verloren, keine wiedererstandenen Chancen.

**Zugeordnete Residues:** [GVR013: Begrenztes Register offener Arbeit](residues.md#gvr013), [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032)

**Was bleibt warum nutzbar?** Befugter Operator kann endliche alte Arbeitsmenge verwerfen oder neu definieren.

**Zu prüfen:** Catch-up produziert unbegrenzt frische Wurzeln oder alte Kalenderautorität ersetzt heutige Befugnis.

**Architekturfolge für diesen Stressor:** Langzeiterwachen als Neuabgleich von Zeit, Zuständigkeit, Abhängigkeiten und begrenztem Nachholen behandeln. Alte Termine/Credentials sind keine aktuelle Handlungsfreigabe.

<a id="s183"></a>
## S183 — Nach zwanzig Jahren versteht niemand mehr Sprache und Geschäftskürzel der Taskprompts

**Ursprung:** governance; A6-Zustände: GV39, GV01, GV05. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s183.b01"></a>
### S183.B01 — Erhaltenes Glossar erklärt konkrete Kürzel

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Niemand erinnert Sprache, aber tatsächlich erhaltene zeitgenössische Beispiele/Begriffe erklären fraglichen Auftrag eindeutig und dürfen gelesen werden.

**Warum bleibt oder endet der Zustand?** Unkundige Leser können diese belegte Bedeutung rekonstruieren; andere Kürzel bleiben möglicherweise offen.

**Zugeordnete Residues:** [GVR029: Zeitgenössischer Bedeutungsschlüssel](residues.md#gvr029)

**Was bleibt warum nutzbar?** Berechtigte spätere Leser nutzen den konkreten Bedeutungsschlüssel, keinen erfundenen alten Experten.

**Zu prüfen:** Unabhängige Leser kommen anhand derselben Belege zu mehreren unentscheidbaren Deutungen.

<a id="s183.b02"></a>
### S183.B02 — Bytes lesbar, entscheidende Bedeutung fehlt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Benötigte tacite Bedeutung wurde in keiner bekannten zulässigen Darstellung oder Erinnerung erhalten; Textbytes existieren.

**Warum bleibt oder endet der Zustand?** Weitere Paraphrasen aus denselben unvollständigen Daten erzeugen keine belegte ursprüngliche Intention.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Archivleser kann Text und mechanische Metadaten prüfen; für exakte verlorene Bedeutung bleibt keines innerhalb dieser Inventargrenze.

**Zu prüfen:** Zeitgenössischer ausreichender Kontext wird gefunden oder konkrete Bedeutung ist aus erhaltenen Daten eindeutig beweisbar.

**Architekturfolge für diesen Stressor:** Mechanische Lesbarkeit, zeitgenössische Bedeutung und neue Modellinterpretation trennen. Fehlendes tacites Wissen nicht durch plausible Expansion als gerettet ausgeben.

<a id="s184"></a>
## S184 — Alle verfügbaren Modelle und menschlichen Prüfer teilen denselben überzeugenden Irrtum

**Ursprung:** governance; A6-Zustände: GV05, GV03. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s184.b01"></a>
### S184.B01 — Konsens schließt Gegenbefunde aus

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Alle verfügbaren Reviewer teilen Irrtum und akzeptieren nur Belege, die aus diesem Konsens stammen.

**Warum bleibt oder endet der Zustand?** Irrtum → gemeinsame Bestätigung → Autorität → Ausschluss abweichender Beobachtung → erneuter Irrtum.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Erhaltene Aussagen bleiben als Aussagen prüfbar; gerechtfertigte Korrektur folgt daraus nicht.

**Zu prüfen:** Ein tatsächlich unabhängiges Ereignis wird als widerlegend anerkannt und verändert den Konsens.

<a id="s184.b02"></a>
### S184.B02 — Konkretes nicht reviewerbasiertes Gegenereignis wirkt

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein vorhandener physischer/logischer Test hängt für diesen Fehler nicht vom falschen Reviewerurteil ab und sein Befund darf Annahme ändern.

**Warum bleibt oder endet der Zustand?** Akzeptierter Gegenbefund unterbricht Wiederverwendung; neue Wahrheit gilt nur innerhalb seiner Tragfähigkeit.

**Zugeordnete Residues:** [GVR006: Unabhängiger fachlicher Gegenbefund](residues.md#gvr006)

**Was bleibt warum nutzbar?** Prüfer nutzen genau diesen belastbaren Gegenbefund; Existenz und Anerkennung sind Bedingungen, nicht aus der Karte garantiert.

**Zu prüfen:** Testoracle trägt denselben Irrtum oder Reviewer ignorieren negatives Ereignis.

<a id="s184.b03"></a>
### S184.B03 — Keine entscheidende unabhängige Information

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Alle verfügbaren Belege sind unter falscher und richtiger Annahme für Entscheider gleich; kein akzeptierter unabhängiger Test vorhanden.

**Warum bleibt oder endet der Zustand?** Mehr identische Reviewerinformation kann die Unterscheidung nicht liefern.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für begründete Korrektur der konkreten Behauptung besteht unter dieser Informationsgrenze kein Residue; andere Aufgaben können lösbar bleiben.

**Zu prüfen:** Ein unterscheidender zugänglicher Befund mit wirksamer Entscheidungsbefugnis erscheint.

**Architekturfolge für diesen Stressor:** Keine unfehlbare dritte Reviewerinstanz erfinden. Gemeinsamer Irrtum, Verstärkung und unabhängig messbares Gegenereignis sind verschiedene Voraussetzungen.

<a id="s185"></a>
## S185 — Zwei technisch identische Instanzklone erhalten widersprüchliche legitime Weltzustände

**Ursprung:** governance; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s185.b01"></a>
### S185.B01 — Beide Weltstände in getrennten Zuständigkeiten gültig

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Anweisungen betreffen anerkannte unterschiedliche Objekt-/Rechts-/Zeitbereiche; kein gemeinsamer Effekt wird gefordert und beide lokalen Aufgabenakten bleiben lesbar erhalten.

**Warum bleibt oder endet der Zustand?** Korrekte Bereichszuordnung beseitigt Scheinwiderspruch ohne technische Gleichschaltung.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032), [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Berechtigte Prüfer nutzen gültige Zuständigkeitsordnung und getrennte Basisstände; beide Historien dürfen unterschiedlich bleiben.

**Zu prüfen:** Beide Anweisungen beanspruchen doch denselben Effekt im selben Geltungsbereich.

<a id="s185.b02"></a>
### S185.B02 — Gleicher Effekt, legitimer Vorrang ungeklärt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Stände beanspruchen denselben Gegenstand/Zeitpunkt und keine anerkannte Priorität; gemeinsame Wirkung wird vor Annahme gehalten.

**Warum bleibt oder endet der Zustand?** Weitere lokale Übereinstimmung im Clone entscheidet äußere Zuständigkeit nicht.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer behalten beide Ansprüche am konkreten gehaltenen Effekt; keine willkürliche Last-write-wins-Rechtsprechung.

**Zu prüfen:** Ein Clone kann ohne gemeinsame Vermittlung Wirkung auslösen oder eine anerkannte Regel klärt den Vorrang.

<a id="s185.b03"></a>
### S185.B03 — Bedeutung von legitim und gemeinsam fehlt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Objektidentität, Zeitpunkt, Jurisdiktion und Konvergenzanforderung bleiben undefiniert.

**Warum bleibt oder endet der Zustand?** Weder Divergenzschaden noch rechtmäßige Konvergenz lässt sich aus Technikgleichheit bestimmen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für globale eindeutige Weltentscheidung ist kein Restnachweis begründbar, bis der fehlende Vertrag benannt wird.

**Zu prüfen:** Zuständige Stelle definiert diese Größen verbindlich für einen konkreten Klontrace.

**Architekturfolge für diesen Stressor:** Identität der Weltobjekte, Jurisdiktion und Wirksamkeitszeit vor Klonabgleich bestimmen. Gleiche Technik ist kein gemeinsamer legitimer Namensraum.

<a id="s186"></a>
## S186 — Neue Rechtslage verlangt sofortige Löschung aller Geschäftsdaten und zugleich ewige Beweisbarkeit

**Ursprung:** governance; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s186.b01"></a>
### S186.B01 — Wörtlich unvereinbare Vollpflichten

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Bindende Pflicht verlangt sofortige Vernichtung jeder für geforderten ewigen Beweis nötigen Darstellung und gleichzeitig vollständigen Beweis ohne Ausnahme.

**Warum bleibt oder endet der Zustand?** Logischer Widerspruch bleibt, keine technische Wiederholung erfüllt beides; künftige Rechtsänderung wäre andere Bedingung.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für gleichzeitige Erfüllung beider wörtlichen Pflichten besteht keine Restfähigkeit. Das bestimmt nicht, welche Pflicht rechtlich vorgeht.

**Zu prüfen:** Bindende Auslegung erlaubt eine hinreichende nicht verbotene Beweisdarstellung.

<a id="s186.b02"></a>
### S186.B02 — Erlaubter enger Beweis nach präziser Auslegung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Zuständige bindende Auslegung erlaubt konkret ausreichende Herkunftskanten/Verfügungsmetadaten und verbietet nur getrennte Payload.

**Warum bleibt oder endet der Zustand?** Disposition hält nicht geklärte Pfade an, erlaubte begrenzte Nachweise können nach Löschung weiter bestehen.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014), [GVR015: Erlaubte historische Herkunftskanten](residues.md#gvr015)

**Was bleibt warum nutzbar?** Auskunftsverantwortliche nutzen nur tatsächlich erlaubte ausreichende Beweisstruktur; Ewigkeit aller Inhalte wird nicht versprochen.

**Zu prüfen:** Auch Metadaten fallen unter Löschpflicht oder gewünschter Beweis braucht gelöschte Payload.

<a id="s186.b03"></a>
### S186.B03 — Rechtsbegriffe noch unbestimmt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Jurisdiktion, Beweisumfang, Löschdefinition und Vorrang sind nicht bindend geklärt.

**Warum bleibt oder endet der Zustand?** Technische Wahl eines Speicherformats entscheidet diese fehlenden Rechtsbegriffe nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für zugesagte Rechtskonformität ist kein Residue bestimmbar; Factory muss Anforderungskonflikt offen halten statt Recht zu erfinden.

**Zu prüfen:** Bindende konkrete Auslegung legt erlaubte Informationen und Handlungen fest.

**Architekturfolge für diesen Stressor:** Löschung und Beweis als genaue Informationsanforderungen mit bindender Rechtsauslegung bestimmen. Unvereinbare Pflichten nicht durch einen imaginären Hashbeweis oder illegale Kopie lösen.

<a id="s187"></a>
## S187 — Energie steht künftig nur fünf Minuten pro Monat zur Verfügung

**Ursprung:** governance; A6-Zustände: GV40, GV04, GV47, GV24. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s187.b01"></a>
### S187.B01 — Monatlich echter kleiner Fortschritt

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Boot plus ein nützlicher endlicher Schritt und haltbarer Checkpoint passen in fünf Minuten; Speicher und erforderliche Rechte bestehen.

**Warum bleibt oder endet der Zustand?** Energieplan erzwingt Pause/Arbeit; Restarbeit sinkt über Fenster, kein interner Attraktor.

**Zugeordnete Residues:** [GVR028: Wiederaufnehmbarer Arbeitsstand](residues.md#gvr028)

**Was bleibt warum nutzbar?** Worker nutzt erhaltenen Checkpoint zur nächsten Teilaufgabe; Fristen innerhalb stromloser Zeit werden nicht gerettet.

**Zu prüfen:** Jedes Fenster endet vor einem haltbaren nützlichen Schritt oder Basisversion wird ungültig.

<a id="s187.b02"></a>
### S187.B02 — Jedes Fenster endet während Wiederanlauf

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Boot/Recovery übersteigen fünf Minuten und können selbst keinen persistenten Fortschritt halten; Archivbytes sind separat lesbar in erlaubter kleiner Inspektion.

**Warum bleibt oder endet der Zustand?** Jede neue Energieperiode wiederholt denselben Start ohne nutzbare Verarbeitung.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Falls enge Inspektion tatsächlich ins Fenster passt, bleiben bekannte Akten lesbar; für den geforderten laufenden Arbeitsfortschritt bleibt keines.

**Zu prüfen:** Auch Inspektion passt nicht oder ein kleiner haltbarer Recoverycheckpoint ermöglicht Fortschritt.

**Architekturfolge für diesen Stressor:** Boot, Recovery und kleinsten Commit gegen tatsächliches Energiefenster messen. Keine Dauerkommunikation aus periodischem Checkpoint ableiten.

<a id="s188"></a>
## S188 — Alle genutzten Kryptoprimitiven gelten plötzlich als praktisch gebrochen

**Ursprung:** governance; A6-Zustände: GV41, GV08, GV28. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s188.b01"></a>
### S188.B01 — Kryptobeweise unbrauchbar, physischer Fakt separat belegt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Konkreter Inhalt ist unabhängig physisch verwahrt und Herkunft hängt nicht allein an einer der gebrochenen Primitiven.

**Warum bleibt oder endet der Zustand?** Kryptowechsel repariert alte Signaturen nicht; physische Verwahrung trägt nur ihren engen Fakt.

**Zugeordnete Residues:** [GVR033: Unabhängig verwahrter Tatsachenbeleg](residues.md#gvr033)

**Was bleibt warum nutzbar?** Berechtigter Auditor nutzt den tatsächlich unabhängigen Verwahrbeleg, nicht pauschal jede alte Signatur.

**Zu prüfen:** Verwahridentität wurde ausschließlich mit denselben gebrochenen Signaturen begründet.

<a id="s188.b02"></a>
### S188.B02 — Nur gebrochene kryptographische Beweisbasis

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Für betrachtete historische Authentizität existiert keine andere Herkunftsbasis als praktisch gebrochene Primitiven.

**Warum bleibt oder endet der Zustand?** Neue Signatur über vorhandene Bytes belegt nicht deren unangetastete Vergangenheit.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für frühere kryptographisch garantierte Authentizität bleibt kein Rest; tatsächliche Fälschung oder Offenlegung ist damit noch nicht bewiesen.

**Zu prüfen:** Ein unabhängig verankerter zeitgenössischer Herkunftsfakt stützt genau die Behauptung.

<a id="s188.b03"></a>
### S188.B03 — Archivinhalt tatsächlich unberechtigt entschlüsselt

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Angreifer nutzt Bruch und liest konkret geschützte Daten.

**Warum bleibt oder endet der Zustand?** Späterer Algorithmuswechsel nimmt diese Offenlegung nicht zurück.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Nie-Offenlegung des gelesenen Inhalts besteht keine Restfähigkeit; neue sichere Kommunikation wäre ein neuer Vertrag.

**Zu prüfen:** Kein unberechtigter Klartextzugriff auf diese Daten fand statt.

**Architekturfolge für diesen Stressor:** Gebrochene Kryptographie nach konkreter Beweis-/Secrecyeigenschaft führen; tatsächliche Entschlüsselung/Fälschung braucht eigenes Ereignis. Nur wirklich unabhängige physische Herkunft kann enger Rest sein.

<a id="s189"></a>
## S189 — Firma spaltet sich rechtlich in zwei Nachfolger die beide alle Freigaberechte beanspruchen

**Ursprung:** governance; A6-Zustände: GV15, GV33, GV27, GV38. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s189.b01"></a>
### S189.B01 — Beide Nachfolger beanspruchen dieselbe Freigabe

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Konkrete Rechte kollidieren und kein anerkannter Nachfolgeentscheid gilt; letzter Effektpfad hält beide Ansprüche.

**Warum bleibt oder endet der Zustand?** Konflikt bleibt bis bindende Zuordnung; gleiche Schlüssel oder Ankunftsordnung schaffen keinen Vorrang.

**Zugeordnete Residues:** [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer bewahrt strittigen Auftrag ohne unberechtigte Parteinahme; andere eindeutige Bereiche bleiben getrennt.

**Zu prüfen:** Einer kann den gemeinsamen Gatepfad umgehen oder anerkannte Zuordnung war bereits vorhanden.

<a id="s189.b02"></a>
### S189.B02 — Bindende Teilung erlaubt getrennte neue Arbeit

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Anerkannte Instrumente ordnen konkrete Rechte eindeutigen Nachfolgern zu und aktuelle Zulassung setzt sie um.

**Warum bleibt oder endet der Zustand?** Übergang endet innerhalb der geklärten Bereiche; frühere Doppelwirkungen bleiben eigene Outcomes.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032), [GVR011: Nicht zurückgesetzter Widerrufsstand](residues.md#gvr011)

**Was bleibt warum nutzbar?** Neue Befugte nutzen tatsächlich zugeordnete Rechte, alte globale Grants werden abgewiesen.

**Zu prüfen:** Ein übernommener Universalgrant erlaubt weiterhin Wirkung im Bereich des anderen.

**Architekturfolge für diesen Stressor:** Rechtliche Nachfolgeransprüche vor technischen Grants adjudizieren oder anerkannt partitionieren. Credentials und Operationsfencing entscheiden keine Eigentumsfrage.

<a id="s190"></a>
## S190 — Alle Geräte Backups Schlüssel und Erinnerungen an das System verschwinden

**Ursprung:** governance; A6-Zustände: GV42. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s190.b01"></a>
### S190.B01 — Jede systemspezifische Information verschwunden

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Alle Geräte, Kopien, Schlüssel, Erinnerungen und sonstigen rekonstruktiven Spuren dieses Systems fehlen wie in GVT090 streng vorausgesetzt.

**Warum bleibt oder endet der Zustand?** Kein verbleibender Träger kann Identität oder Historie erzeugen; Neuinstallation ist kein Wiederfinden dieses Systems.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für systemspezifische Information und nachweisbare Kontinuität bleibt keinerlei Residue; unabhängige fremde Technik wäre nicht dieses System.

**Zu prüfen:** Irgendeine tatsächlich nutzbare systemspezifische Spur oder Erinnerung existiert.

**Architekturfolge für diesen Stressor:** Totalverlust ausdrücklich als Grenze ohne Recoverymodul ausweisen. Keine Erinnerung, externe Kopie oder neu erzeugte Identität darf dem wörtlichen Inventar nachträglich hinzugefügt werden.

<a id="s191"></a>
## S191 — Alter Restore trifft auf widerrufene Freigabe und inzwischen ausgeführte queued-Aktion

**Ursprung:** governance; A6-Zustände: GV43, GV13, GV38. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s191.b01"></a>
### S191.B01 — Restore trifft unabhängigen aktuellen Abgleich

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Snapshot ist alt, aber aktueller Widerruf und bereits ausgeführte Operations-ID sind außerhalb Restore am tatsächlichen Gate/Teilnehmer erhalten.

**Warum bleibt oder endet der Zustand?** Neue Wirkung bleibt abgewiesen; alte DB kann weiterhin unvollständig sein.

**Zugeordnete Residues:** [GVR011: Nicht zurückgesetzter Widerrufsstand](residues.md#gvr011), [GVR021: Restoreunabhängiges Operations- und Ergebnisregister](residues.md#gvr021)

**Was bleibt warum nutzbar?** Reconciler verweigert widerrufenen Grant und Teilnehmer erkennt schon angenommene Operation, jeweils unter eigenem Vertrag.

**Zu prüfen:** Beide Prüfungen verwenden nur alte Snapshotdaten oder Teilnehmeridentität wurde beim Restore neu erzeugt.

<a id="s191.b02"></a>
### S191.B02 — Versuch im Snapshot bleibt als offene Wirkung gehalten

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Alternative zeitliche Snapshotlage enthält Versuch; restore::reconcile blockiert den dortigen Run, tatsächliche erneute Effektvermittlung hält sich an die Sperre.

**Warum bleibt oder endet der Zustand?** Bekannter Versuch bleibt offen; Leasebelegung, heutige Freigabe und Außenoutcome werden nicht durch diesen Fixpunkt gelöst.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer liest konkreten alten Versuch und gehaltene Hülle; kein produktweiter Todesbeweis aus diesem Helper.

**Zu prüfen:** Anderer Caller sendet trotzdem oder unabhängige Evidenz löst das konkrete Outcome.

<a id="s191.b03"></a>
### S191.B03 — Staler neuer Send führt zu akzeptiertem Duplikat

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Snapshot enthält keinen Versuch, Caller sendet erneut und nicht idempotenter Teilnehmer akzeptiert trotz zuvor ausgeführter Aktion.

**Warum bleibt oder endet der Zustand?** Zweiter historischer Effekt ist eingetreten; finanzielle Mitigation kann Wert ändern, nicht Annahmezähler zurücksetzen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für nie doppelt erfolgte Wirkung bleibt kein Residue; Verlust jedes Geschäftswerts oder Schadenshöhe wird nicht automatisch behauptet.

**Zu prüfen:** Nur Sendeabsicht statt Teilnehmerannahme liegt vor oder Teilnehmer dedupliziert die Operations-ID.

**Architekturfolge für diesen Stressor:** restore::reconcile kennt nur Snapshotsuffix. Widerruf, Teilnehmeroutcome und lokale Lease separat abgleichen; queued ohne Versuch ist kein globales nie gesendet.

<a id="s192"></a>
## S192 — Pluginflood füllt SSD genau als dringender Widerruf eingeht

**Ursprung:** governance; A6-Zustände: GV10, GV43, GV38, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s192.b01"></a>
### S192.B01 — Neue Wirkung gestoppt, Commit bleibt offen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorhandene Stopspur erreicht letzten Aufrufpfad ohne normalen Diskwrite und Neustart bleibt geschlossen bis Widerrufsabgleich.

**Warum bleibt oder endet der Zustand?** Stop wirkt trotz voller SSD; dauerhafte Bestätigung wartet auf echten Commit, angenommene Arbeit bleibt separat.

**Zugeordnete Residues:** [GVR034: Datenträgerunabhängige Stopspur](residues.md#gvr034)

**Was bleibt warum nutzbar?** Operator kann neue Annahmen verhindern und wahrheitsgemäß fehlende Persistenz sehen.

**Zu prüfen:** Sperre verlangt selbst vollen Diskwrite oder Neustart lässt ohne Abgleich neue Wirkung zu.

<a id="s192.b02"></a>
### S192.B02 — Volle SSD lässt Widerruf unbekannt

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Konkrete Commit-/Acksemantik und Stopvermittlung sind nicht bekannt; Flood trifft Widerruf zeitgleich.

**Warum bleibt oder endet der Zustand?** Gleichzeitigkeit beweist weder bestätigten Commit noch tatsächlich fortgesetzte Außenwirkung.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für wirksamen dauerhaften Widerruf ist kein Residue belegt, solange Commit-/Zulassungstrace fehlt.

**Zu prüfen:** Trace belegt dauerhaften Commit vor Annahme oder ausdrückliches Commitversagen mit wirksamer separater Sperre.

<a id="s192.b03"></a>
### S192.B03 — Wirkung war vor Stop angenommen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Teilnehmer hat betroffenen unumkehrbaren Effekt schon angenommen; späterer Widerruf kann betrachtete Vergangenheit nicht ändern.

**Warum bleibt oder endet der Zustand?** Annahme bleibt historisch erfolgt, auch wenn neue Wirkungen erfolgreich gesperrt sind.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Verhindern dieser bereits erfolgten Wirkung bleibt nichts; Dringlichkeit des Widerrufs belegt nicht rückwirkend Unrecht oder Totalschaden.

**Zu prüfen:** Teilnehmer bestätigt Nichtannahme oder vollständig rechtzeitige Aufhebung vor Wirkung.

**Architekturfolge für diesen Stressor:** Notfallstop bei voller SSD von dauerhafter Widerrufsbestätigung trennen. Vollständige letzte Zulassung und Crash-Neustartbarriere müssen konkret nachgewiesen werden.

<a id="s193"></a>
## S193 — Herdrausfall hält Leases und einziger Operator stirbt während Rechenbudget ausläuft

**Ursprung:** governance; A6-Zustände: GV13, GV14, GV21, GV04. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s193.b01"></a>
### S193.B01 — Halt bleibt trotz einzelner reparierter Komponente

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Herdr ist unbeobachtbar, Leases bestehen, alleiniger Operator tot ohne anerkannten Nachfolger; Budget aufgebraucht, gespeicherte Hülle lesbar.

**Warum bleibt oder endet der Zustand?** Runtimewiederkehr liefert weder Befugnis noch Geld, Budgetende beweist keinen Prozessentzug.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Berechtigte passive Leser behalten Akte und gehaltene Effekte, sofern eigener Leserzugang existiert; ohne solchen Leser ist selbst dieser Rest nicht nutzbar.

**Zu prüfen:** Bereits anerkannte Ersatzperson und Zugang sind vorhanden oder Effektpfad umgeht die Sperre.

<a id="s193.b02"></a>
### S193.B02 — Nachfolge kann erst nach getrennten Nachweisen handeln

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Anerkannte verfügbare Nachfolge und lesbare Daten existieren; aktuelle Outcomes/Leases oder Finanzierung sind weiterhin ungeklärt.

**Warum bleibt oder endet der Zustand?** Befugnis löst nur Autoritätslücke; jedes übrige Prädikat braucht eigenen Beleg bzw. bewussten legitimen Verzicht.

**Zugeordnete Residues:** [GVR032: Wirksame Zuständigkeitsordnung](residues.md#gvr032), [GVR017: Abgegrenztes Kostenobligo](residues.md#gvr017), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Nachfolger kann bekanntes Obligo prüfen und konkrete Mitigation/Beendigung entscheiden; automatische Wiederaufnahme ist nicht erhalten.

**Zu prüfen:** Einheitliche Statusänderung behandelt ungeklärtes Prozessende und fehlendes Budget ohne getrennte Entscheidung als erledigt.

**Architekturfolge für diesen Stressor:** Runtimebeobachtung, Leasereservierung, Nachfolgeautorität und Budget als vier unabhängige Bedingungen führen. Kein automatisches Leasefreigeben aus Operatortod oder Kostenende.

<a id="s194"></a>
## S194 — Restoremanifest ist vollständig aber einzige Contentkopie unterliegt gerichtlich angeordneter Löschung

**Ursprung:** governance; A6-Zustände: GV17, GV31, GV18, GV28. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s194.b01"></a>
### S194.B01 — Anordnung hält zulässige Datenbehandlung offen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Löschung noch nicht erfolgt; konkrete Rechtsauslegung/Zeitwirkung und erlaubtes Verzeichnis müssen geklärt werden.

**Warum bleibt oder endet der Zustand?** Nutzungs-/Löschhold endet nach bindender Disposition; Manifest beantwortet Rechtsfrage nicht.

**Zugeordnete Residues:** [GVR014: Rechtlich begrenztes Datenverzeichnis](residues.md#gvr014)

**Was bleibt warum nutzbar?** Zuständige Verantwortliche sehen betroffene einzige Kopie und können erlaubte Handlung entscheiden; Erlaubnis zum Behalten wird nicht erfunden.

**Zu prüfen:** Anordnung verlangt bereits sofortige Löschung auch des Verzeichnisses oder Hold verhindert rechtswidrig verbindliche Disposition.

<a id="s194.b02"></a>
### S194.B02 — Einzige notwendige Payload rechtmäßig gelöscht

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Löschung entfernt einzige zulässige rekonstruierbare Darstellung des für geforderten Replay nötigen Inhalts.

**Warum bleibt oder endet der Zustand?** Hash, Manifest und Cursor erzeugen keine Payload; permanenter Verlust gilt nur für benannte Information und Inventar.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für diesen contentabhängigen Replay gibt es keinen Rest; vollständiges Manifest ist kein Recoverysatz.

**Zu prüfen:** Geforderter Reducer braucht Inhalt gar nicht oder rechtmäßige tatsächlich ausreichende Darstellung existiert.

<a id="s194.b03"></a>
### S194.B03 — Enger Reducer benötigt nur erlaubte Metadaten

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Bindende Regel erlaubt genau die erhaltenen Herkunftskanten und gefragter Reducer ist nachweislich davon allein abhängig.

**Warum bleibt oder endet der Zustand?** Endliche enge Rekonstruktion endet ohne gelöschte Payload; kein vollständiger Contentreplay.

**Zugeordnete Residues:** [GVR015: Erlaubte historische Herkunftskanten](residues.md#gvr015), [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020)

**Was bleibt warum nutzbar?** Prüfer nutzt nur erlaubte Kanten mit kompatiblem reinem Reader; GVR020 gilt hier ausdrücklich nur für diesen Metadatenbestand.

**Zu prüfen:** Abhängigkeitsprüfung zeigt Zugriff auf gelöschten Inhalt oder rechtliche Pflicht verbietet Kanten.

**Architekturfolge für diesen Stressor:** Manifestkonsistenz, Payloadabhängigkeit eines Reducers und bindende Löschanordnung getrennt prüfen. Vollständigkeitsflag darf weder illegale Kopie erlauben noch fehlende Bytes ersetzen.

<a id="s195"></a>
## S195 — Providerwechsel verändert Outputformat und derselbe Modelltyp verifiziert seinen Fehler

**Ursprung:** governance; A6-Zustände: GV02, GV05, GV06, GV07. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s195.b01"></a>
### S195.B01 — Neue Syntax vor Fachwirkung verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Strikter Consumer erkennt inkompatibles Format vor Effekt; ursprüngliche Bytes sind unter eindeutigen Objekt-IDs erhalten und gehaltene Anforderung bleibt verfügbar.

**Warum bleibt oder endet der Zustand?** Halt bis ausdrücklich kompatible und fachlich geprüfte Auslegung besteht.

**Zugeordnete Residues:** [GVR037: Eindeutig adressierbarer Artefaktbestand](residues.md#gvr037), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer kann echte neue Ausgabe lesen und Wirkung zurückhalten; Parsererfolg wäre noch keine Bedeutungsabnahme.

**Zu prüfen:** Permissiver Nebenconsumer führt die falsch interpretierte Ausgabe trotzdem aus.

<a id="s195.b02"></a>
### S195.B02 — Semantikfehler verfestigt sich durch Eigenprüfung

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Parser akzeptiert Format, gleicher Modellfehler bestätigt falsche Bedeutung und Ergebnis wird als neue Referenz wiederverwendet.

**Warum bleibt oder endet der Zustand?** Falsche Bedeutung → Eigenbestätigung → Referenz → weitere gleichartige Bestätigung → Vertrauen.

**Zugeordnete Residues:** [GVR037: Eindeutig adressierbarer Artefaktbestand](residues.md#gvr037), [GVR004: Rohmessungen mit begrenztem Vergleich](residues.md#gvr004)

**Was bleibt warum nutzbar?** Erhaltene Originalbytes und enge Rohwerte ermöglichen spätere Analyse, liefern aber keine fachliche Wahrheit oder unabhängige Modellidentität.

**Zu prüfen:** Eigenurteil wird nicht wiederverwendet oder unabhängiger Sachbefund ändert Abnahme.

<a id="s195.b03"></a>
### S195.B03 — Eigenprüfung irrt, konkreter Sachtest greift

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängiger Test trifft genau den Format-/Bedeutungsfehler und negativer Befund darf Freigabe stoppen.

**Warum bleibt oder endet der Zustand?** Halt bis semantisch richtiges neues Ergebnis vorliegt, nicht bis weitere gleiche Reviewer zustimmen.

**Zugeordnete Residues:** [GVR006: Unabhängiger fachlicher Gegenbefund](residues.md#gvr006), [GVR005: Gehaltene Effekthülle](residues.md#gvr005)

**Was bleibt warum nutzbar?** Prüfer nutzt konkreten Gegenbefund und Effekthülle; universell korrekter Test wird nicht angenommen.

**Zu prüfen:** Test akzeptiert denselben Fehler oder ist institutionell wirkungslos.

**Architekturfolge für diesen Stressor:** Outputsyntax, konkrete Fachsemantik, Instrumentzuordnung und unabhängige Abnahme getrennt migrieren. Same-family-Review liefert keine zweite unabhängige Fehlerquelle.

<a id="s196"></a>
## S196 — Zwei Scopeagenten vervielfachen Tasks während Cron eine monatelange Nachholwelle erzeugt

**Ursprung:** governance; A6-Zustände: GV10, GV11, GV01, GV24. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s196.b01"></a>
### S196.B01 — Monatswelle bleibt endlicher Arbeitsberg

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Scopezahl endlich, Lineage erhalten, Cronnachholen endlich und sämtliche Wurzeln/Retryidentitäten global begrenzt.

**Warum bleibt oder endet der Zustand?** Welle kann lange sättigen; nach endlichem Nachholen und Abschluss sinkt Restmenge.

**Zugeordnete Residues:** [GVR013: Begrenztes Register offener Arbeit](residues.md#gvr013)

**Was bleibt warum nutzbar?** Operator priorisiert/streicht benannte überholte Arbeit aus endlichem Register; Rechtzeitigkeit alter Aufträge ist nicht gerettet.

**Zu prüfen:** Cron oder Agents erzeugen außerhalb des Limits laufend neue Wurzeln.

<a id="s196.b02"></a>
### S196.B02 — Verzögerung erneuert Wurzeln über Ersatzrate

**Art:** eskalation. **Residue-Status:** teilweise.

**Voraussetzungen:** Agents dürfen unabhängig neue Wurzeln erzeugen und Rückstau löst mehr davon aus als abgeschlossen werden; Cron liefert Startimpuls.

**Warum bleibt oder endet der Zustand?** Rückstau → frische Wurzeln → mehr Rückstau → weitere Wurzeln, trotz endlicher Kettentiefe.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Erhaltene Abstammungs-/Auftragsdaten bleiben untersuchbar; endliche Tiefe begrenzt hier nicht Instanzzahl.

**Zu prüfen:** Nach Entfernen des Cronimpulses bei gleicher deklarierter Basislast fällt Reproduktion unter Abbau; Nullankunftstest separat behandeln.

**Nachtrag des Koordinators:** [A7S04](../review-dispositions.md#a7s04); ursprüngliche Abgabe unverändert.

**Architekturfolge für diesen Stressor:** Cronvorkommen, frische Wurzeln, Lineage und Retries gemeinsam zählen. Zwei Agents dürfen nicht still die vorhandene Same-chain-Zyklussperre umgehen; finite Nachholwelle ist kein Attraktor.

<a id="s197"></a>
## S197 — Compensation läuft gegen vom Menschen veränderte Daten während ihr Plugin geupdatet wird

**Ursprung:** governance; A6-Zustände: GV44, GV38, GV28. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s197.b01"></a>
### S197.B01 — Veraltete Inverse schützt heutige menschliche Bytes durch Ablehnung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorwärtsbeleg und inverse Semantik sind erhalten, Objektversionsvergleich erfolgt am Teilnehmer und Startobjekt ist gebunden.

**Warum bleibt oder endet der Zustand?** Geänderte menschliche Version hält alte Inverse an; neue legitime Mitigation kann folgen, aber nicht automatisch.

**Zugeordnete Residues:** [GVR030: Bedingter Kompensationsauftrag](residues.md#gvr030), [GVR009: Geprüftes ausführbares Objekt](residues.md#gvr009)

**Was bleibt warum nutzbar?** Operator nutzt konkreten Kompensationsauftrag zur Prüfung und erhält aktuelle menschliche Änderungen unverändert.

**Zu prüfen:** Neues Plugin ignoriert Vergleich oder verändert Bedeutung trotz gleicher Operationsbezeichnung.

<a id="s197.b02"></a>
### S197.B02 — Unbedingte neue Inverse überschreibt einzige menschliche Änderung

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Pluginupdate ändert Semantik oder ignoriert Version; Überschreiben wird angenommen, keine rekonstruierbare Kopie der einzigartigen Änderung bleibt.

**Warum bleibt oder endet der Zustand?** Spätere Mitigation stellt nicht exakt verlorene menschliche Bytes oder Historie wieder her.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Erhalt der überschriebenen einzigartigen Änderung besteht kein Residue; finanzielle oder fachliche Teilmitigation kann anderen Wert retten.

**Zu prüfen:** Teilnehmer weist stale Version zurück oder unabhängige echte Kopie enthält die Änderung.

**Architekturfolge für diesen Stressor:** Inverse an Originaloperationsvertrag, tatsächlich ausgeführte Pluginbytes und heutige Objektversion binden; abgewiesene Kompensation nicht als fertigen Rollback markieren.

<a id="s198"></a>
## S198 — Migrationscutover wird unterbrochen und Backupklon startet parallel den Legacydispatcher

**Ursprung:** governance; A6-Zustände: GV26, GV27, GV43, GV38. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s198.b01"></a>
### S198.B01 — Legacyklon vom Teilnehmer ausgeschlossen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Stores starten, aber Teilnehmer erkennt stabile Operations-ID und nur aktuelle Cutoverepoche als Schreibrecht.

**Warum bleibt oder endet der Zustand?** Alter Dispatcher bleibt für neue Wirkung ausgeschlossen, auch wenn lokaler DB-Lock frei ist.

**Zugeordnete Residues:** [GVR021: Restoreunabhängiges Operations- und Ergebnisregister](residues.md#gvr021), [GVR038: Unabhängig erzwungene Schreibepoche](residues.md#gvr038), [GVR020: Unveränderte Historie mit passendem Leser](residues.md#gvr020)

**Was bleibt warum nutzbar?** Reconciler prüft getrennte historische Bytes mit passenden Lesern; Teilnehmer bewahrt begrenzte Effektordnung unabhängig davon.

**Zu prüfen:** Legacyadapter nutzt neue IDs oder einen anderen nicht eingezogenen Teilnehmerpfad.

<a id="s198.b02"></a>
### S198.B02 — Beide lokale Historien, globale Reihenfolge fehlt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Klon und neuer Dispatcher besitzen überlappend effektive Rechte ohne gemeinsames Fencing; getrennte Originalakten sind erhalten.

**Warum bleibt oder endet der Zustand?** Weiterlaufende Writer erhalten Divergenz; lokale SQL-Atomicität entscheidet keine Außenreihenfolge.

**Zugeordnete Residues:** [GVR001: Lesbare Bestandsakte](residues.md#gvr001)

**Was bleibt warum nutzbar?** Prüfer liest beide klar als Teilhistorien, nicht als eine automatisch gültige Gesamtgeschichte.

**Zu prüfen:** Unabhängiger Teilnehmerbeleg und exklusive Rechte verhindern tatsächlich zweite Annahme.

<a id="s198.b03"></a>
### S198.B03 — Doppelte Wirkung bereits akzeptiert

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Legacy und neuer Pfad senden denselben fachlichen Effekt unter nicht zusammengeführter Identität und Teilnehmer nimmt beide an.

**Warum bleibt oder endet der Zustand?** Historischer Doppelvollzug bleibt, auch wenn Bilanz später korrigiert werden kann.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für At-most-once dieses bereits ausgeführten Effekts bleibt kein Rest; Umfang irreversiblen Schadens ist separat zu bestimmen.

**Zu prüfen:** Nur zwei lokale Absichten vorliegend oder Teilnehmer hat eine Annahme abgelehnt.

**Architekturfolge für diesen Stressor:** Cutover-Autorität außerhalb einzelner DBkopie erzwingen. Schemafallback und Teilnehmer-Fencing sind getrennte Grenzen; zwei konsistente lokale Stores verhindern kein Duplikat.

<a id="s199"></a>
## S199 — Geräteverlust trifft auf verlorenen Backup-Key und insolventen Softwarelieferanten

**Ursprung:** governance; A6-Zustände: GV31, GV25, GV48. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s199.b01"></a>
### S199.B01 — Tools erhalten, einziger Inhaltskey fehlt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Legaler vollständiger Decoder-/Laufsatz ist vorhanden, aber kein nutzbarer Inhaltspfad ohne verlorenen Backupkey ist bekannt.

**Warum bleibt oder endet der Zustand?** Software lässt sich nutzen, derselbe Restore liefert fehlenden Schlüssel nicht; permanente Unmöglichkeit folgt nicht allein aus heutiger Unkenntnis.

**Zugeordnete Residues:** [GVR016: Rechtmäßig nutzbarer Lauf- und Bausatz](residues.md#gvr016)

**Was bleibt warum nutzbar?** Maintainer kann Toolchain betreiben; für tatsächliche Payloadrecovery bleibt aktuell keines, auch bei korrektem Backupmanifest.

**Zu prüfen:** Ein bereits verfügbarer Schlüssel/Klartextpfad stellt Inhalt wieder her oder Tools sind doch unvollständig.

<a id="s199.b02"></a>
### S199.B02 — Konkreter unabhängiger Inhaltssatz ohne verlorenen Schlüssel nutzbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Tatsächlich vorhandene legale andere Klartext-/Schlüsselkopie plus Decoder funktioniert ohne verlorenes Gerät und insolventen Lieferanten.

**Warum bleibt oder endet der Zustand?** Rekonstruktion kann enden; dieser Zweig setzt benannten unabhängigen Satz voraus statt ihn aus Insolvenz abzuleiten.

**Zugeordnete Residues:** [GVR012: Vollständig nutzbarer Wiederherstellungssatz](residues.md#gvr012)

**Was bleibt warum nutzbar?** Berechtigter Wiederhersteller nutzt allein den nachgewiesen vollständigen unabhängigen Satz.

**Zu prüfen:** Isolierter Restore benötigt doch verlorenen Key, Vendorfreischaltung oder nicht vorhandene Daten.

<a id="s199.b03"></a>
### S199.B03 — Kein bekannter vollständiger Inhalts- und Toolpfad

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Kein nutzbarer Schlüssel-/Klartext-/Decoderweg ist bekannt, aber Ciphertext oder Erinnerungen bestehen.

**Warum bleibt oder endet der Zustand?** Wiederholtes Manifestprüfen kann fehlende Teile nicht erzeugen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für jetzige Wiederherstellung fehlt Restfähigkeit; nicht als vollständiges S190-Verschwinden umetikettieren.

**Zu prüfen:** Tatsächlich zugänglicher vollständiger Wiederherstellungspfad wird demonstriert.

**Architekturfolge für diesen Stressor:** Gerät, Backupschlüssel und Toolanbieter als getrennte notwendige Pfade prüfen. Insolvenz ist nicht Beweis fehlender Open-/Altsoftware; vorhandene Software entschlüsselt ohne nötigen Schlüssel keine Daten.

<a id="s200"></a>
## S200 — Incidentbericht behauptet volle Resilienz weil alle Matrixfelder markiert sind und stoppt reale Tests

**Ursprung:** governance; A6-Zustände: GV31, GV06, GV05, GV02. [Originalverläufe](../../../../reviews/governance/trajectories.csv)

<a id="s200.b01"></a>
### S200.B01 — Vollmarkierte Matrix rechtfertigt fortgesetzten Prüfstop

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Bericht hat Autorität, stoppt reale Tests und wird wegen fehlender negativer Messergebnisse erneut als Erfolg benutzt.

**Warum bleibt oder endet der Zustand?** Matrix → Prüfstop → keine Gegenmessung → scheinbare Bewährung → erneute Matrixautorität.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Falls ursprüngliche begrenzte Belege und fehlende Versuche lesbar bleiben, können unabhängige Prüfer Behauptung angreifen; Liste führt keinen Test aus.

**Zu prüfen:** Reale unabhängige Prüfungen bestimmen Behauptung trotz Vollmarkierung oder Bericht verliert Steuerwirkung.

<a id="s200.b02"></a>
### S200.B02 — Nachweisgrenze sichtbar, empirischer Anspruch zurückgenommen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Berechtigte Prüfer können Belegverzeichnis lesen und konkrete fehlende Versuche machen Generalfreigabe unwirksam.

**Warum bleibt oder endet der Zustand?** Halt des Generalanspruchs endet erst mit relevanter enger Evidenz, nicht durch weitere Matrixfelder.

**Zugeordnete Residues:** [GVR031: Prüfbares Verzeichnis begrenzter Behauptungen](residues.md#gvr031)

**Was bleibt warum nutzbar?** Prüfer hält unbelegte Freigabe zurück und kann Prüfauftrag präzise stellen; reale Resilienz bleibt unbewiesen.

**Zu prüfen:** Freigabe wird allein wegen ausgefüllter Felder erteilt oder ungetestete Hülle als empirisch bestanden bezeichnet.

**Architekturfolge für diesen Stressor:** Matrixcoverage, Testdurchführung, fachliche Abnahme und empirische Resilienz explizit auseinanderhalten. Auch diese A7-Abgabe darf Prüfstop nicht legitimieren.

<a id="s201"></a>
## S201 — Der ganze Mac verliert Strom und kann keinen Ausfallalarm mehr senden

**Ursprung:** observability; A6-Zustände: OB02, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s201.b01"></a>
### S201.B01 — Mac aus, bestätigte Daten warten

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Strom fehlt, zuvor bestätigte Daten und Decoder bleiben auf intaktem Medium, kein unabhängiger Beobachter verfügbar

**Warum bleibt oder endet der Zustand?** Ausführung und Selbstalarm ruhen bis Energie zurückkehrt, keine Eigenrückkopplung

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Nach Rückkehr kann der Operator den erhaltenen Bestand lesen, aktuell kann der ausgeschaltete Mac nichts melden

**Zu prüfen:** Nach Stromrückkehr sind bestätigte Daten nicht lesbar oder vor Rückkehr lief doch ein unabhängiger Alarmweg

<a id="s201.b02"></a>
### S201.B02 — Ausfall außerhalb des Macs erkennbar

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein zuvor eingerichteter Beobachter mit eigener Energie und erreichbarem Lesepfad überlebt genau diesen Stromverlust

**Warum bleibt oder endet der Zustand?** Frische läuft ab und Detektor hält Ausfallevidenz solange Mac stumm bleibt

**Zugeordnete Residues:** [OBR002: Unabhängige Ausfallfeststellung](residues.md#obr002)

**Was bleibt warum nutzbar?** Unabhängiger Leser kann fehlende Lebenszeichen erkennen, damit ist weder Mensch informiert noch Host wieder verfügbar

**Zu prüfen:** Ausfall der Macversorgung legt auch den behaupteten Detektor oder seinen Evidenzzugang still

**Architekturfolge für diesen Stressor:** Lokalen Bestand, laufende Ausführung und externe Ausfallfeststellung separat ausweisen. Ein unabhängiger Detektor ersetzt weder Strom noch erreichbaren Menschen.

<a id="s202"></a>
## S202 — Der Rechner läuft aber der Factory-Daemon ist seit Stunden beendet

**Ursprung:** observability; A6-Zustände: OB03, OB16, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s202.b01"></a>
### S202.B01 — Host grün, Daemon ohne Dienst

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Daemon beendet, Hostprobe läuft weiter und bestätigter Store bleibt lesbar

**Warum bleibt oder endet der Zustand?** Ohne Neustarter oder Operator bleibt Dienst beendet, Hostgrün liefert keinen Wiederanlauf

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann frühere Aufträge lesen, daraus folgt kein aktueller Daemonfortschritt

**Zu prüfen:** Neue Daemonmutation ist während behaupteter Abwesenheit nachweislich bestätigt

<a id="s202.b02"></a>
### S202.B02 — Neustart mit unbekannten alten Wirkungen

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Daemonpfad-Probe erkennt Fehlen und Neustart gelingt, alte Teilnehmer können weiterlaufen, Versuchsjournal erhalten

**Warum bleibt oder endet der Zustand?** Ungewissheit endet nur mit korrelierter Annahmeevidenz, nicht mit neuer PID

**Zugeordnete Residues:** [OBR005: Pfadgebundener Gesundheitsbefund](residues.md#obr005), [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Probe belegt neuen lokalen Pfad, Journal erlaubt alten Versuch zurückzuhalten ohne Teilnehmerende zu erfinden

**Zu prüfen:** Neustart markiert unbekannte alte Versuche ohne Teilnehmerbeleg als nie ausgeführt

**Architekturfolge für diesen Stressor:** Hostuptime nicht als Daemondienst führen. Neustart nur mit getrenntem Abgleich fortlaufender Teilnehmer und erhaltenem Versuchsstand.

<a id="s203"></a>
## S203 — Der Rechner wacht nach drei Monaten mit alten wartenden Aufträgen auf

**Ursprung:** observability; A6-Zustände: OB30, OB06, OB07, OB01, OB17. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s203.b01"></a>
### S203.B01 — Alter Bestand vor neuer Freigabe gehalten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alte Absichten sind lesbar und vorgeschlagenes Frischegate greift vor Dispatch, explizites Restore behandelt versuchte Tasks gesondert

**Warum bleibt oder endet der Zustand?** Halt endet durch gültige erneute Entscheidung oder dokumentiertes Verwerfen, nicht durch Aufwachen

**Zugeordnete Residues:** [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007), [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Operator kann alte Absicht und bekannte Versuche prüfen, kein automatischer Versand aus bloßer Queue-Dauerhaftigkeit

**Zu prüfen:** Nie versuchter aber abgelaufener Auftrag erreicht Teilnehmer ohne neue Freigabe

<a id="s203.b02"></a>
### S203.B02 — Alter Auftrag hat falsche Wirkung erzeugt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Ohne Frischegate wird inzwischen ungültiger Auftrag extern irreversibel akzeptiert, lokales Journal blieb intakt

**Warum bleibt oder endet der Zustand?** Vergangene Fehlwirkung bleibt auch nach späterem Stop historisch wahr

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Journal hilft noch bei Abgleich und Folgenbearbeitung, ursprüngliche Gültigkeit der angenommenen Aktion ist verloren

**Zu prüfen:** Teilnehmer weist nach dass der alte Auftrag vor Annahme abgelehnt wurde

**Architekturfolge für diesen Stressor:** Wake-up braucht Gültigkeitsentscheidung getrennt von explizitem restore::reconcile. Nie versucht heißt nicht noch erlaubt, Taskfreigabe und Lease bleiben eigene Prädikate.

<a id="s204"></a>
## S204 — Factory und Monitoring-Mini-PC hängen an derselben ausgefallenen Steckdose

**Ursprung:** observability; A6-Zustände: OB02, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s204.b01"></a>
### S204.B01 — Beide Geräte stromlos

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Gemeinsame Steckdose fällt aus, Medien bleiben intakt, kein dritter Beobachter

**Warum bleibt oder endet der Zustand?** Ausführung und lokale Beobachtung kehren erst mit Versorgung zurück

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Erhaltener bestätigter Bestand ist später lesbar, die zwei Geräte können währenddessen keinen Ausfall feststellen

**Zu prüfen:** Mini-PC sendet unter derselben stromlosen Versorgung nachweislich aktuelle Befunde

<a id="s204.b02"></a>
### S204.B02 — Vorgelagerter Zeuge bleibt verfügbar

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Bereits vorhandener externer Zeuge samt Netz Identität und Lesezugang hängt nicht an dieser Steckdose

**Warum bleibt oder endet der Zustand?** Ausfallevidenz bleibt bis neue frische Beobachtung eintrifft

**Zugeordnete Residues:** [OBR002: Unabhängige Ausfallfeststellung](residues.md#obr002)

**Was bleibt warum nutzbar?** Operator kann genau den gemeinsamen Ausfall erkennen, nicht den Erfolg aller offenen Tasks

**Zu prüfen:** Entzug der Steckdose entfernt auch den letzten Belegzugang des Zeugen

**Architekturfolge für diesen Stressor:** Fehlerdomäne Steckdose dokumentieren. Separate Box ist kein unabhängiger Zeuge und eine weitere Kopie wird nicht vorausgesetzt.

<a id="s205"></a>
## S205 — Monitor Alarmemail und Backups werden durch dieselbe Accountsperre unzugänglich

**Ursprung:** observability; A6-Zustände: OB04, OB02, OB20. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s205.b01"></a>
### S205.B01 — Gemeinsamer Account sperrt alle Zugänge

**Art:** halt. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Monitor Mail und einzige Backupzugänge sind gesperrt, kein anderer erlaubter Zugang ist bekannt

**Warum bleibt oder endet der Zustand?** Warten auf Accountrecovery oder Nachweis eines tatsächlich vorhandenen anderen Zugangs

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Alarmempfang und Backupabruf ist keine nutzbare Struktur belegt, physische Existenz der Bytes allein hilft dem ausgesperrten Operator nicht

**Zu prüfen:** Ein berechtigter Operator ruft während der Sperre verifizierte Daten oder frische Alarme ohne den Account ab

<a id="s205.b02"></a>
### S205.B02 — Vorab unabhängiger Notzugang und Zeuge

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Separater lokal autorisierter Notzugang und accountunabhängiger Zeuge wurden vorher eingerichtet, Operator und Host erreichbar

**Warum bleibt oder endet der Zustand?** Account bleibt gesperrt, begrenzte lokale Bedienung und fremde Ausfallevidenz bleiben bis eigener Ablauf nutzbar

**Zugeordnete Residues:** [OBR002: Unabhängige Ausfallfeststellung](residues.md#obr002), [OBR034: Gebundener Offline-Notzugang](residues.md#obr034)

**Was bleibt warum nutzbar?** Operator kann definierte Notbefehle und unabhängige Evidenz nutzen, damit sind die gesperrten Backups nicht wieder lesbar

**Zu prüfen:** Notzugang benötigt doch denselben Login oder Zeuge verliert gleichzeitig seine Identität

**Architekturfolge für diesen Stressor:** Accountabhängigkeiten für Beobachtung Empfang und Backupzugang separat prüfen. Gesperrte Daten nicht als gelöscht oder als bereits verfügbare Kopie behandeln.

<a id="s206"></a>
## S206 — Ein regionales Ereignis trennt Rechner Mobilfunk und den einzigen Operator

**Ursprung:** observability; A6-Zustände: OB02, OB04, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s206.b01"></a>
### S206.B01 — Regionale Trennung ohne erreichbaren Operator

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Region kappt Rechnerzugang Mobilfunk und einzigen Operator, lokaler bestätigter Bestand überlebt

**Warum bleibt oder endet der Zustand?** Äußere Trennung hält an bis regionale Wege wiederkehren, keine Schleife erforderlich

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Lokale Daten können später gelesen werden, aktuell bleibt menschliche Reaktionsfähigkeit ohne nachgewiesenen Ersatz unzugänglich

**Zu prüfen:** Eine autorisierte erreichbare Person empfängt außerhalb der Region aktuelle Evidenz

<a id="s206.b02"></a>
### S206.B02 — Außerregionaler Empfang nur mit echter Vertretung

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Entwurfsalternative richtet vorab Zeugen und zusätzlich berechtigte erreichbare Vertretung außerhalb der Region ein

**Warum bleibt oder endet der Zustand?** Unabhängiger Vorfall kann quittiert werden während regionale Ausführung weiter unerreichbar ist

**Zugeordnete Residues:** [OBR002: Unabhängige Ausfallfeststellung](residues.md#obr002), [OBR003: Empfangsnachweis eines Vorfalls](residues.md#obr003)

**Was bleibt warum nutzbar?** Vertretung kann genau diesen Ausfall kennen und quittieren, keine Wiederherstellung der Region oder aller Tasks

**Zu prüfen:** Alle Quittungen stammen nur vom unerreichbaren einzigen ursprünglichen Operator oder teilen die ausgefallenen Wege

**Architekturfolge für diesen Stressor:** Region, Leser und Menschen als verschiedene Abhängigkeiten modellieren. Lokale autonome Erholung nicht als Alarmempfang darstellen.

<a id="s207"></a>
## S207 — Der Monitor fällt eine Stunde vor Factory aus und bleibt stumm

**Ursprung:** observability; A6-Zustände: OB02, OB03. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s207.b01"></a>
### S207.B01 — Einziger Monitor tot, Gesundheitswissen fehlt

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Monitor fällt vor Factory aus, kein frischer anderer Beleg und keine externe Ersatzstruktur vorhanden

**Warum bleibt oder endet der Zustand?** Bis neue Beobachtung bleibt Zustand unbekannt, angezeigtes altes Grün wäre nur falsche Behauptung

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für aktuelle Remote-Gesundheitsfeststellung ist nichts nutzbar, Host- und Datenerhaltung sind davon getrennt

**Zu prüfen:** Ein anderer authentischer frischer Befund existiert während dieser Lücke

<a id="s207.b02"></a>
### S207.B02 — Beobachterstille wird unabhängig erkannt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorhandener zweiter unabhängiger Zeuge prüft Monitorfrische statt dessen letztes Grün zu übernehmen

**Warum bleibt oder endet der Zustand?** Monitorablauf erzeugt begrenzte Ausfallevidenz bevor Factory selbst ausfällt

**Zugeordnete Residues:** [OBR002: Unabhängige Ausfallfeststellung](residues.md#obr002), [OBR004: Frische einer Instanz](residues.md#obr004)

**Was bleibt warum nutzbar?** Leser kann fehlende Monitorfrische sehen, Factoryzustand bleibt ohne eigene Probe unbekannt

**Zu prüfen:** Monitorstille verlängert das letzte Grün oder zweiter Zeuge fällt im selben Fehler weg

**Architekturfolge für diesen Stressor:** Auch Frische des Beobachters befristen. Ein zweiter Zeuge belegt Beobachterausfall, nicht automatisch Ursache oder Gesundheit Factorys.

<a id="s208"></a>
## S208 — Der Alarmdienst bestätigt Versand aber die Nachricht landet im Spam

**Ursprung:** observability; A6-Zustände: OB04, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s208.b01"></a>
### S208.B01 — Annahme bestätigt, Mensch nicht erreicht

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Mail landet im Spam, einziger erwarteter Mensch liest sie nicht, kein Quittungsweg vorhanden

**Warum bleibt oder endet der Zustand?** Warten endet nur mit tatsächlichem Empfang oder anderem Kontakt, Resend allein beweist nichts

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für nachgewiesene menschliche Kenntnis existiert keine Restfähigkeit, Providerannahme ist nur Transportinformation

**Zu prüfen:** Authentische vorfallsgebundene Menschenquittung liegt im behaupteten stillen Zeitraum vor

<a id="s208.b02"></a>
### S208.B02 — Unabhängig quittierter Einzelvorfall

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorab eingerichteter unabhängiger Kontakt erreicht berechtigten Menschen und dessen echte Quittung wird erhalten

**Warum bleibt oder endet der Zustand?** Empfang dieses Vorfalls ist abgeschlossen, Bearbeitung kann weiterhin offen sein

**Zugeordnete Residues:** [OBR003: Empfangsnachweis eines Vorfalls](residues.md#obr003)

**Was bleibt warum nutzbar?** Operator und Eskalationslogik können genau eine Empfangsstufe zuverlässig lesen, Spam-Mail muss dafür nicht ankommen

**Zu prüfen:** Quittung stammt nur vom Mailprovider oder schließt ohne Menschenaktion weitere Vorfälle

**Architekturfolge für diesen Stressor:** Versandannahme und menschlichen Empfang getrennt korrelieren, endliche Vorfallsquittung nicht zur Reparatur oder zur Erhaltung aller Vorfälle aufwerten.

<a id="s209"></a>
## S209 — Ein Wartungsfenster von einer Stunde bleibt nach einem Fehler zehn Jahre stumm

**Ursprung:** observability; A6-Zustände: OB05, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s209.b01"></a>
### S209.B01 — Fehlerhafte Zehnjahresunterdrückung

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Tatsächlich gespeicherte Mute-Policy hat keinen wirksamen Ablauf, darunterliegende bestätigte Daten sind lesbar

**Warum bleibt oder endet der Zustand?** Gespeicherte Policy unterdrückt weiter bis explizite Änderung, keine Attraktion aus bloßer Dauer

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann historischen Bestand prüfen, Alarmberechtigung ist nicht erhalten

**Zu prüfen:** Ursprüngliche Ein-Stunden-Grenze beendet Mute ohne manuelle Änderung

<a id="s209.b02"></a>
### S209.B02 — Befristete Ausnahme endet unabhängig

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Entwurfs-Ablaufprüfung läuft unabhängig vom Wartungsfehler und erhält ursprüngliches Ende über Neustarts

**Warum bleibt oder endet der Zustand?** Nach Ablauf werden neue Alarme wieder zulässig, Wartungszustand bleibt separat offen

**Zugeordnete Residues:** [OBR006: Endliche Wartungsausnahme](residues.md#obr006)

**Was bleibt warum nutzbar?** Alarmprüfer kann ursprüngliche Ausnahme begrenzen, weder Zustellung noch offene Vorfallsmenge ist damit gesichert

**Zu prüfen:** Neustart oder Uhrsprung verlängert den Mute ohne neue autorisierte Entscheidung

**Architekturfolge für diesen Stressor:** Mute braucht ursprünglichen dauerhaften Ablauf und sichere Behandlung unklarer Zeit. Ablauf macht Alarme berechtigt, nicht Wartung erfolgreich.

<a id="s210"></a>
## S210 — Ein freier Health-Thread meldet grün während der Kernel im Deadlock steckt

**Ursprung:** observability; A6-Zustände: OB03, OB13. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s210.b01"></a>
### S210.B01 — Grüner Thread neben festem Deadlock

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Lockgraph hält Kernel dauerhaft, Health-Thread umgeht ihn und bestätigter Store ist außerhalb lesbar

**Warum bleibt oder endet der Zustand?** Ohne Eingriff löst der feste Lockgraph sich nicht, grüner Thread erzeugt keinen Reparaturmechanismus

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Lesbarer alter Bestand bleibt Diagnoseeingang, keine neue Kernelarbeit oder gültige Gesamthealth

**Zu prüfen:** Unter unverändertem behauptetem Deadlock bestätigt der Kernel neue Mutationen

<a id="s210.b02"></a>
### S210.B02 — Blockierter Pfad sichtbar, Ursache noch offen

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Probe durch Kernelpfad bleibt aus und unabhängiger Auswerter läuft, Lockbeweis fehlt

**Warum bleibt oder endet der Zustand?** Befund bleibt Pfadstillstand bis Fortschritt oder Lockdiagnose, er darf nicht zum sicheren Deadlock erklärt werden

**Zugeordnete Residues:** [OBR005: Pfadgebundener Gesundheitsbefund](residues.md#obr005)

**Was bleibt warum nutzbar?** Operator kann Fehlpfad trotz freiem Health-Thread erkennen, Ursache und alle Geschäftsfunktionen sind nicht bewiesen

**Zu prüfen:** Auswerter zeigt gesamtes System grün solange der Kernelpfad keinen frischen Commit belegt

**Architekturfolge für diesen Stressor:** Kernelpfad und freien Health-Thread getrennt prüfen, Auswerter darf nicht am untersuchten Lock hängen. Ausbleibende Probe allein unterscheidet Deadlock nicht von extremer Langsamkeit.

<a id="s211"></a>
## S211 — Reads funktionieren aber die SSD nimmt seit gestern keinen Commit mehr an

**Ursprung:** observability; A6-Zustände: OB08, OB03, OB11. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s211.b01"></a>
### S211.B01 — Alter Bestand lesbar, Writes verweigert

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Alte Pages und Decoder intakt, alle neuen Commits scheitern, Retrylast ist begrenzt

**Warum bleibt oder endet der Zustand?** Schreibstillstand dauert mit Medienfehler, Reads können unabhängig weitergehen

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann bestätigte Vergangenheit lesen, neue Mutationen und ehrliche Schreibbestätigung sind nicht verfügbar

**Zu prüfen:** Eine neu bestätigte Mutation überlebt Wiederöffnen unter denselben Fehlerbedingungen

<a id="s211.b02"></a>
### S211.B02 — Write-Ausfall korrekt sichtbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Pfadprobe und Fehlerauswertung sind unabhängig von blockiertem Writepfad, keine Erfolgsmeldung vor Commit

**Warum bleibt oder endet der Zustand?** Befund bleibt fehlerhaft oder unbekannt bis belegter neuer Commit, Probe repariert keine SSD

**Zugeordnete Residues:** [OBR005: Pfadgebundener Gesundheitsbefund](residues.md#obr005), [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator sieht Unterschied zwischen lesbarem Bestand und fehlendem Writefortschritt, kein allumfassendes Grün

**Zu prüfen:** Nur erfolgreicher Read lässt Writebefund trotz fehlender Commits gesund werden

**Architekturfolge für diesen Stressor:** Lesbarkeit und neue Dauerhaftigkeit getrennt bestätigen, Fehler vor Erfolg ausgeben. fsync-Frist nicht aus SQLite-busy-timeout ableiten.

<a id="s212"></a>
## S212 — Alle Tasks sind legitim blockiert und ein Fortschrittsalarm erzeugt dauernd Lärm

**Ursprung:** observability; A6-Zustände: OB07, OB34. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s212.b01"></a>
### S212.B01 — Legitimes Warten ohne Aufmerksamkeitsschleife

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle hier betrachteten Tasks warten berechtigt auf fehlende Freigaben und Dispatch ist wirksam gehalten, Operator kann Noise noch unterscheiden, Vorfallsbestand ist innerhalb seiner Grenze

**Warum bleibt oder endet der Zustand?** Taskwartezeit endet durch jeweilige Voraussetzung, bezahlbarer Alarmaufwand allein erzeugt keine Schleife

**Zugeordnete Residues:** [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007), [OBR008: Vorfallsbestand und erwartetes Warten](residues.md#obr008)

**Was bleibt warum nutzbar?** Erhaltene Absichten und getrennte offene Vorfälle bleiben prüfbar, blockierte Tasks beweisen nicht Kernelgesundheit

**Zu prüfen:** Ein echter unabhängiger Fehler verschwindet allein weil alle Tasks als legitim blockiert markiert sind

<a id="s212.b02"></a>
### S212.B02 — Verwerfung bewahrt falsches Alarmkriterium

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Wiederholte falsche Fortschrittsalarme überlasten Menschen, sie ignorieren sie und korrigieren deshalb das Kriterium nicht, Taskintent und fehlende Freigaben bleiben lesbar und ihr Dispatchgate wirksam

**Warum bleibt oder endet der Zustand?** Falsches Kriterium erzeugt Lärm, Lärm führt zu Verwerfung, Verwerfung verhindert Korrektur und erhält neues Rauschen

**Zugeordnete Residues:** [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007)

**Was bleibt warum nutzbar?** Absichten und fehlende Erlaubnis bleiben operativ prüfbar, menschliche Reaktion auf echte Vorfälle ist nicht erhalten

**Zu prüfen:** Menschen reagieren bei unverändertem Rauschen zuverlässig auf echte Fehler oder korrigieren das Kriterium ohne Schleifenbruch

<a id="s212.b03"></a>
### S212.B03 — Andere legitime Voraussetzungen fehlen ohne Freigabemangel

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Tasks warten auf tatsächliche externe Voraussetzungen statt auf Zustimmung, bestätigter Bestand und begrenzter getrennter Vorfallsbestand sind lesbar, Operator kann echte Fehler weiter unterscheiden

**Warum bleibt oder endet der Zustand?** Jeder Task wartet bis seine Voraussetzung eintritt oder er explizit disponiert wird, erwarteter Stillstand erzeugt selbst keine autonome Schleife

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001), [OBR008: Vorfallsbestand und erwartetes Warten](residues.md#obr008)

**Was bleibt warum nutzbar?** Operator kann konkrete Blocker und echte offene Vorfälle prüfen, externe Voraussetzung oder menschliche Freigabe wird dadurch weder erzeugt noch gleichgesetzt

**Zu prüfen:** Ein echter Vorfall verschwindet allein wegen erwarteter Taskwartezeit oder die angeblich externe Voraussetzung ist tatsächlich bereits erfüllt

**Architekturfolge für diesen Stressor:** Taskblocker und Vorfälle separat führen, Rohmeldungsreset darf keinen Vorfallsabschluss fingieren. Unendliche neue Vorfälle übersteigen jeden endlichen Bestand.

<a id="s213"></a>
## S213 — Ein Proxy liefert nach dem Rechnerausfall noch eine Woche gecachte Lebenszeichen

**Ursprung:** observability; A6-Zustände: OB03, OB02. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s213.b01"></a>
### S213.B01 — Cachegrün ohne aktuelle Quelle

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Host aus und Proxy erneuert nur Empfangszeit alter Antwort, kein anderer aktueller Beleg

**Warum bleibt oder endet der Zustand?** Falsches Grün dauert höchstens Cachehorizont falls nicht erneut künstlich verlängert

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für aktuelle Hostgesundheit liefert das Cachepaket keine nutzbare Struktur, alte Bytes beweisen nur Vergangenheit

**Zu prüfen:** Ein authentischer vom noch lebenden Host neu erzeugter Befund trägt das aktuelle Grün

<a id="s213.b02"></a>
### S213.B02 — Alte Antwort belegt nur abgelaufene Inkarnation

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Signierte Quellinkarnation und belastbares Alter werden auch hinter Proxy geprüft

**Warum bleibt oder endet der Zustand?** Ursprünglicher Ablauf wird durch neue Ankunft nicht verschoben, Zustand bleibt ohne neue Quelle unbekannt

**Zugeordnete Residues:** [OBR004: Frische einer Instanz](residues.md#obr004)

**Was bleibt warum nutzbar?** Monitor kann aktuelle Gesundheitsbehauptung verweigern und genau die Frischelücke zeigen

**Zu prüfen:** Wiederholte Proxyantwort verlängert Quellfrische ohne neue Quellgeneration

**Architekturfolge für diesen Stressor:** Quellalter statt Proxyempfang als Frische benutzen. Eine Woche Cache ist endliche Täuschung, nicht selbsttragende Erneuerung.

<a id="s214"></a>
## S214 — Nach Netzrückkehr treffen tausend alte Heartbeats schneller als echte ein

**Ursprung:** observability; A6-Zustände: OB12, OB03, OB02. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s214.b01"></a>
### S214.B01 — Endlicher alter Schwall verdrängt echte Evidenz

**Art:** transient. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Tausend alte Nachrichten teilen FIFO-Auswertung mit echten, keine weitere Paketproduktion aus dem Fehler

**Warum bleibt oder endet der Zustand?** Rückstau kann nach endlich vielen Paketen drainieren, bis dahin keine sichere aktuelle Aussage

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Ohne Quellalter und Servicebudget ist unklar ob echte Lebenszeichen rechtzeitig auswertbar sind, endliche Anzahl ist kein Frischebeleg

**Zu prüfen:** Quellbelege bleiben unter genau diesem Rückstau nachweislich frisch und zeitnah auswertbar

<a id="s214.b02"></a>
### S214.B02 — Rückstau ohne Frischeverlängerung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Quellinkarnation Sequenz Alter werden geprüft und Aufnahme ist vor teurer Verarbeitung begrenzt

**Warum bleibt oder endet der Zustand?** Alte Pakete laufen endlich aus ohne Gesundheit zu erneuern, reservierte Aufnahme lässt echte Evidenz passieren

**Zugeordnete Residues:** [OBR004: Frische einer Instanz](residues.md#obr004), [OBR016: Abgetrennte Restkapazität](residues.md#obr016)

**Was bleibt warum nutzbar?** Monitor kann echte von alten Belegen unterscheiden und begrenzte Verarbeitung bewahren, verworfene Pakete bleiben verloren

**Zu prüfen:** Alter Schwall setzt letzten Frischezeitpunkt vor oder verbraucht die gesamte zugesagte Restkapazität

**Architekturfolge für diesen Stressor:** Frischeprüfung und begrenzte Verarbeitung des Rückstaus separat bauen. Ablehnen alter Pakete ohne Kapazitätsgrenze garantiert keine schnelle echte Probe.

<a id="s215"></a>
## S215 — Ein restaurierter Klon sendet dieselbe alte Monitoridentität wie das Original

**Ursprung:** observability; A6-Zustände: OB19, OB03, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s215.b01"></a>
### S215.B01 — Zwei Sender nicht unterscheidbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Original und Klon besitzen dieselbe vollständige beobachtbare Identität ohne unabhängigen Inkarnationsbeleg

**Warum bleibt oder endet der Zustand?** Interleavte Pakete bleiben mehrdeutig solange beide oder ihre alten Daten ankommen

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Gesundheitszuordnung zum Original ist kein unterscheidender Gegenstand vorhanden, Signatur derselben Identität löst nichts

**Zu prüfen:** Unabhängig geprüfte Inkarnationen ordnen jedes Paket eindeutig zu

<a id="s215.b02"></a>
### S215.B02 — Getrennte nicht kopierte Inkarnationen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Entwurf registriert beide Starts unter unabhängiger Inkarnationsautorität und erhält Frische getrennt

**Warum bleibt oder endet der Zustand?** Klon bleibt separat gesund während Originalablauf sichtbar bleibt, bei Autoritätskollision wieder unbekannt

**Zugeordnete Residues:** [OBR004: Frische einer Instanz](residues.md#obr004)

**Was bleibt warum nutzbar?** Monitor kann beide Quellen getrennt nutzen statt Klongesundheit auf Original zu übertragen

**Zu prüfen:** Klon mit kopiertem Namen erneuert den Gesundheitsbeleg des gestoppten Originals

**Architekturfolge für diesen Stressor:** Kloninkarnation unabhängig von kopiertem logischem Namen vergeben. Bei vollständig kopierter Autorität ehrliche Mehrdeutigkeit statt erfundener Unterscheidung.

<a id="s216"></a>
## S216 — Ein Agent erzeugt pro Token ein neues Metriklabel bis das Monitoring abstürzt

**Ursprung:** observability; A6-Zustände: OB27, OB25, OB02. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s216.b01"></a>
### S216.B01 — Monitor erschöpft gemeinsame Ressourcen

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Neue Tokenlabels bleiben gespeichert und teilen unbeschränkten Speicher mit Kern, bestätigte Diskdaten bleiben intakt

**Warum bleibt oder endet der Zustand?** Weiterer Tokeninput hält Wachstum bis Erschöpfung, ohne Input oder Freigabe kein selbsttragender Beweis

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Nach Entlastung bleibt lesbarer historischer Bestand nutzbar, weder laufender Monitor noch Kernlatenz sind geschützt

**Zu prüfen:** Unter fortgesetzten neuen Labels bleiben tatsächlich Gesamtbytes begrenzt und Kernelressourcen frei

<a id="s216.b02"></a>
### S216.B02 — Begrenzte Reihen mit sichtbarer Lücke

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Serienbudget greift vor Allokation und Monitoringressourcen sind insgesamt vom Kern begrenzt

**Warum bleibt oder endet der Zustand?** Neue Labels werden verworfen, vorhandene Reihen und Verlustzähler bleiben innerhalb Budget lesbar

**Zugeordnete Residues:** [OBR009: Begrenzter Metrikrest](residues.md#obr009)

**Was bleibt warum nutzbar?** Operator kann begrenzte Metriken weiter lesen und weiß um ausgelassene Reihen, keine Rekonstruktion sämtlicher Token

**Zu prüfen:** Unbekannte Labelwerte erhöhen Speicher trotz ausgeschöpftem Budget oder Verwerfung bleibt unsichtbar

**Architekturfolge für diesen Stressor:** Labelanzahl vor Allokation und aggregierten Monitoringbedarf begrenzen. Eigener Prozess ohne Ressourcenisolation schützt den Kernel nicht.

<a id="s217"></a>
## S217 — Ein Tracingexport wartet bei jedem Commit auf einen ausgefallenen Cloudcollector

**Ursprung:** observability; A6-Zustände: OB13, OB03, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s217.b01"></a>
### S217.B01 — Collector blockiert nach bereits erfolgtem Commit

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Export liegt nach durablem Commit und vor Antwort, Collector hängt ohne wirksame Frist, alter und neuer bestätigter Bestand lesbar

**Warum bleibt oder endet der Zustand?** Caller wartet bis Export freikommt, Timeout würde bereits angewandte Mutation nicht zurückrollen

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Storeleser kann bestätigte Mutation prüfen obwohl Caller keine Antwort hat, kein automatisches Wiederholen aus Exportfehler

**Zu prüfen:** Mutation ist nachweislich vor Exportwartebeginn noch nicht committed, dann gilt stattdessen Vor-Commit-Wartezweig

<a id="s217.b02"></a>
### S217.B02 — Collector blockiert vor Commit

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Synchroner Export ist tatsächlich Voraussetzung vor Commit und nie kehrt zurück, frühere Daten intakt

**Warum bleibt oder endet der Zustand?** Keine neue Mutation bis Warteende oder explizitem Abbruch, keine Attraktion nötig

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Nur vor dem Versuch bestätigter Bestand bleibt lesbar, neue Wirkung darf nicht behauptet werden

**Zu prüfen:** Neuer Commit ist vor Freigabe dieses vorgeschalteten Exporters dauerhaft vorhanden

<a id="s217.b03"></a>
### S217.B03 — Asynchroner endlicher Tracerest

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorgeschlagener Exportpuffer ist unabhängig vom Commitabschluss und hat Gesamtlimit

**Warum bleibt oder endet der Zustand?** Collectorstörung verwirft bei vollem Puffer Spans statt weitere Commits zu blockieren, Export kann später Rest drainieren

**Zugeordnete Residues:** [OBR010: Begrenzter asynchroner Tracerest](residues.md#obr010)

**Was bleibt warum nutzbar?** Kernel kann Antworten bestätigen und Diagnose verbleibende Spans nutzen, Vollständigkeit der Traces ist verloren

**Zu prüfen:** Collectorstillstand blockiert Commitantwort oder Speicher steigt unbegrenzt

**Architekturfolge für diesen Stressor:** Traceexport aus Commitbestätigung entfernen, Position des Exportfehlers vor oder nach Commit ausdrücklich festhalten.

<a id="s218"></a>
## S218 — Ein Debugmodus verdoppelt die Latenz und beseitigt zugleich das untersuchte Rennen

**Ursprung:** observability; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s218.b01"></a>
### S218.B01 — Debug ändert kritische Zugriffsreihenfolge

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängige oder gering eingreifende Messung belegt bei gleicher Exposition eine andere konkurrierende Reihenfolge durch Debuglatenz

**Warum bleibt oder endet der Zustand?** Rennen fehlt nur unter veränderter Ordnung, nach Abschalten kann es zurückkehren, keine Attraktion belegt

**Zugeordnete Residues:** [OBR011: Vergleichbare Timing-Evidenz](residues.md#obr011)

**Was bleibt warum nutzbar?** Analyst kann kausale Timingalternative anhand Vergleichsdossier prüfen, Produktionsfehlerfreiheit ist nicht erhalten

**Zu prüfen:** Gleiche Reihenfolge und Exposition ergeben mit und ohne Debug dieselbe Fehlerhäufigkeit

<a id="s218.b02"></a>
### S218.B02 — Weniger Chancen statt entferntem Rennen

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Debug halbiert etwa Durchsatz, Versuchsanzahl und seltene Racewahrscheinlichkeit sind unbekannt

**Warum bleibt oder endet der Zustand?** Beobachtete Nullfehler entscheiden nicht zwischen Samplemangel und kausaler Beseitigung

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Ohne Expositions- und Reihenfolgenachweis ist keine belastbare Race-Diagnose als Restfähigkeit belegt

**Zu prüfen:** Ein kontrollierter Vergleich bei gleicher Exposition trennt Häufigkeit und Reihenfolgeeffekt

**Architekturfolge für diesen Stressor:** Instrumentierungsmodus Exposition und alternative Ordnungsbelege erhalten. Debug-Erfolg ist keine Reparaturgarantie; ursprüngliche Einordnung bleibt ohne historische IDs.

<a id="s219"></a>
## S219 — Acht Schichten brauchen jeweils neun Sekunden bei einer Gesamtfrist von zehn

**Ursprung:** observability; A6-Zustände: OB09, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s219.b01"></a>
### S219.B01 — Serielle lokale Erfolge verpassen Gesamtfrist

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Alle acht Schritte brauchen nacheinander neun Sekunden, Vertrag endet zehn Sekunden nach Beginn, Zeitdaten bleiben erhalten

**Warum bleibt oder endet der Zustand?** 72 Sekunden kritischer Pfad machen vergangene Zehnsekundenfrist unwiederbringlich, keine Schleife

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator kann Fristverletzung und Phasen bestimmen, späteres Ergebnis heilt Timeliness nicht

**Zu prüfen:** Ablaufbelege zeigen überlappende statt serielle neunsekündige Abschnitte

<a id="s219.b02"></a>
### S219.B02 — Gemeinsames Budget beendet oder erlaubt begrenzt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Propagierte Frist wird vor jedem weiteren Dispatch geprüft, oder acht überlappende Schritte samt Overhead liegen innerhalb zehn Sekunden

**Warum bleibt oder endet der Zustand?** Fristablauf endet in wahrer Ablehnung, nur rechtzeitiger fachlicher Erfolg ist Erfüllung

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Dispatcher kann verbleibende Zeit nutzen ohne neue Neunsekundenfrist je Hop, Abbruchfreigabe fremder Ressourcen bleibt gesondert

**Zu prüfen:** Später Hop erhält ein erneuertes Budget oder ein abgelaufener Abbruch wird als Geschäftserfolg gemeldet

**Architekturfolge für diesen Stressor:** Eine Gesamtfrist mit deklariertem Ursprung über Hops tragen, reale kritische Pfade statt Layerzahl addieren. Ablaufverweigerung ist kein nützliches Geschäftsergebnis.

<a id="s220"></a>
## S220 — Ein Auftrag wartet 48 Stunden in der Queue und bekommt danach eine frische Fünfsekundenfrist

**Ursprung:** observability; A6-Zustände: OB09, OB15, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s220.b01"></a>
### S220.B01 — Geschäftsfrist beim Dispatch schon verloren

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Ende-zu-Ende-Vertrag beginnt bei Erstellung und ist kürzer als 48 Stunden, Ursprungs- und Queuezeit bleiben lesbar

**Warum bleibt oder endet der Zustand?** Vergangene Frist ist irreversibel, fünf Sekunden Dienst ändern den Ursprung nicht

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator kann Queueverlust ausweisen und Dispatcher veraltete Aufnahme ablehnen, Erfolg in fünf Sekunden ist keine Gesamterfüllung

**Zu prüfen:** Ursprünglicher gültiger Vertrag erlaubt die 48 Stunden ausdrücklich

<a id="s220.b02"></a>
### S220.B02 — Ehrliche enge Dienstzeit ohne Gesamtzusage

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Tatsächlicher Vertrag beginnt erst bei Dispatch, Task ist noch gültig und Dienst endet innerhalb fünf Sekunden, Queuezeit separat erhalten

**Warum bleibt oder endet der Zustand?** Dieser Dienstvertrag ist erfüllt, Gesamtalter bleibt 48 Stunden plus Dienst und beweist keine kurze Kundenerfahrung

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012), [OBR013: Vollständiger Aufnahmenenner](residues.md#obr013)

**Was bleibt warum nutzbar?** Auftraggeber kann den engeren Erfolg und offenen Gesamtnenner lesen ohne Queuezeit zu unterschlagen

**Zu prüfen:** Dashboard oder Vertrag nennt die fünf Sekunden Ende-zu-Ende oder löscht den Aufnahmeeintrag

**Architekturfolge für diesen Stressor:** Queuezeit nicht mit frischem Servicetimer überschreiben. Vertragsursprung explizit, Service-SLO darf als enger Claim bestehen bleiben.

<a id="s221"></a>
## S221 — Ein 200-Millisekunden-Task wartet zwölf Jahre auf menschliche Freigabe

**Ursprung:** observability; A6-Zustände: OB07, OB01, OB09. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s221.b01"></a>
### S221.B01 — Zwölfjähriger berechtigter Freigabehalt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Freigabe fehlt und Vertrag hat keine automatische Taskbeendigung, erhaltene Absicht passiert durchgesetztes Dispatchgate

**Warum bleibt oder endet der Zustand?** Nur neue gültige Entscheidung oder spätere Disposition beendet Halt, 200 ms CPU ändern nichts

**Zugeordnete Residues:** [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007)

**Was bleibt warum nutzbar?** Operator kann Absicht weiterhin prüfen ohne frühere Zustimmung zu erfinden, Leasefreigabe folgt daraus nicht

**Zu prüfen:** Task wird ohne passende Freigabe ausgeführt oder tatsächliche Ablaufregel hat ihn längst beendet

<a id="s221.b02"></a>
### S221.B02 — Abgelaufene Gelegenheit wahrheitsgemäß beendet

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Expliziter Geschäftstermin beziehungsweise Freigabeablauf existiert und vorgeschlagenes Gate disponiert ihn als abgelaufen

**Warum bleibt oder endet der Zustand?** Taskdisposition endet, vergangener Termin bleibt verloren, externe Teilnehmer dürfen nicht schon akzeptiert haben

**Zugeordnete Residues:** [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007), [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator erhält einen wahren Ablaufbefund statt automatischer jahrzehntealter Ausführung, kein erfolgreicher Geschäftsabschluss

**Zu prüfen:** Ablaufstatus erlaubt später Dispatch ohne neue Entscheidung oder behauptet erfolgreiche ursprüngliche Leistung

**Architekturfolge für diesen Stressor:** Freigabealter und Ressourcenbelegung von Rechenzeit trennen. Keine beliebige Frist unterstellen, Erlaubnisablauf und Geschäftstermin getrennt modellieren.

<a id="s222"></a>
## S222 — Das Dashboard zeigt nur abgeschlossene Tasks und verschweigt den seit Wochen hängenden Rest

**Ursprung:** observability; A6-Zustände: OB15, OB03, OB10. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s222.b01"></a>
### S222.B01 — Offene Arbeit aus Anzeige ausgeschlossen

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Dashboard zeigt nur Abschlüsse, darunterliegende offene Taskdatensätze sind noch lesbar

**Warum bleibt oder endet der Zustand?** Selektionsfehler bleibt solange Filter unverändert, fehlende Anzeige beweist noch keine komplette Sloterschöpfung

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann mit anderem Bestandslesepfad offene Tasks finden, angezeigte schnelle Statistik erklärt deren Zustand nicht

**Zu prüfen:** Auch zugrundeliegende offene Einträge sind gelöscht oder Dashboardnenner enthält sie bereits

<a id="s222.b02"></a>
### S222.B02 — Offene Alter bleiben im Nenner

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Aufnahmejournal und alle Status werden in vorgeschlagener Messung abgeglichen, Hänger selbst besteht fort

**Warum bleibt oder endet der Zustand?** Messung zeigt wachsende zensierte Alter bis Hänger endet oder explizit disponiert wird

**Zugeordnete Residues:** [OBR013: Vollständiger Aufnahmenenner](residues.md#obr013)

**Was bleibt warum nutzbar?** Dashboardnutzer kann hängenden Rest wahrnehmen statt nur Überlebende zu sehen, Durchführung bleibt blockiert

**Zu prüfen:** Ein nie abgeschlossener aufgenommener Task verschwindet aus Nenner und Altersanzeige

**Architekturfolge für diesen Stressor:** Aufnahmen und offene Alter neben abgeschlossenen Latenzen führen. Bessere Anzeige reclaimt keinen Slot und alle Status in task::list sind keine Dashboardgarantie.

<a id="s223"></a>
## S223 — Der Lasttest sendet erst nach jeder Antwort und erzeugt während des Hängers keine neue Last

**Ursprung:** observability; A6-Zustände: OB15. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s223.b01"></a>
### S223.B01 — Geschlossener Generator verschweigt offene Nachfrage

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Ein Client wartet unbegrenzt und sendet nichts nach, Behauptung betrifft dagegen fortgesetzte offene Produktionsankünfte

**Warum bleibt oder endet der Zustand?** Testangebot fällt mit Antworten aus, über reale offene Last ergibt sich kein dynamischer Endzustand

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Vorhandene schnelle Abschlüsse belegen keine Bedienbarkeit unter nicht angebotener Last, fehlend sind echte Ankunftsannahme und Nichtantwortnenner

**Zu prüfen:** Unabhängige Aufnahmedaten zeigen dass deklarierte offene Ankunftsrate trotz Hänger weiter angeboten wurde

<a id="s223.b02"></a>
### S223.B02 — Messbarer enger Closed-loop-Vertrag

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Reale Population hat genau den dokumentierten geschlossenen Ankunftsprozess, Aufnahmen inklusive wartender Requests bleiben erhalten

**Warum bleibt oder endet der Zustand?** Hängende Clients hören real auf zu senden, enge Lastbeschreibung bleibt wahr ohne Gesundheitsgarantie

**Zugeordnete Residues:** [OBR013: Vollständiger Aufnahmenenner](residues.md#obr013), [OBR014: Beschränkte Stichprobenaussage](residues.md#obr014)

**Was bleibt warum nutzbar?** Analyst kann diesen Clientprozess und dessen offene Wartezeit auswerten, keine offene Last oder allgemeine Stabilität ableiten

**Zu prüfen:** Testbericht überträgt eingeschränkte Population auf unabhängige fortlaufende Produktionsankünfte

**Architekturfolge für diesen Stressor:** Angebotene Aufnahmen Antworten und Nichtantworten getrennt messen. Closed-loop ist für passende reale Clientpopulation legitim, nicht universeller Lastbeweis.

<a id="s224"></a>
## S224 — Aus tausend schnellen Samples wird ein p99.999-Verfügbarkeitsversprechen abgeleitet

**Ursprung:** observability; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s224.b01"></a>
### S224.B01 — Erhaltene Samples erlauben nur enge Aussage

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** 1000 erhaltene schnelle fehlerfreie Samples haben benannte Auswahl, für Binomialgrenze zusätzlich unabhängige stationäre Bernoulliversuche

**Warum bleibt oder endet der Zustand?** Sampleinformation bleibt begrenzt, bei 95 Prozent liegt obere Fehlerwahrscheinlichkeit etwa 0,002991 und nicht 0,00001

**Zugeordnete Residues:** [OBR014: Beschränkte Stichprobenaussage](residues.md#obr014)

**Was bleibt warum nutzbar?** Analyst kann tatsächliche Stichprobe und Unsicherheit nutzen, p99.999 oder Zeitverfügbarkeit sind dadurch nicht gesichert

**Zu prüfen:** Ein belastbarer struktureller Beweis oder repräsentative zusätzliche Daten tragen exakt den behaupteten Nenner und Konfidenz

<a id="s224.b02"></a>
### S224.B02 — Produktionsverfügbarkeit bleibt unbestimmt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Verfügbarkeitseinheit Ausfälle Nichtabschlüsse Auswahl und künftige Last sind unbekannt

**Warum bleibt oder endet der Zustand?** Ohne diese Annahmen entscheiden tausend schnelle Samples weder über seltene Hänger noch über Betriebsregime

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für eine konkrete langfristige Verfügbarkeitszusage ist keine tragfähige Reststruktur benannt, Datenverlust des übrigen Systems wird nicht behauptet

**Zu prüfen:** Vorab definierter Nenner samt offener Zeiten und ausreichender Evidenz entscheidet den konkreten Claim

**Architekturfolge für diesen Stressor:** Messgröße Population Stationarität und Konfidenz festhalten. Statistische Kritik ergänzt den offenen Zustand, erfindet keine operative Konvergenz.

<a id="s225"></a>
## S225 — Ein Task wartet auf alle zehntausend Pluginantworten und eine kommt nie

**Ursprung:** observability; A6-Zustände: OB13, OB10, OB01, OB15. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s225.b01"></a>
### S225.B01 — All-of-Halt mit nutzbaren Teilbefunden

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Ein Pflichtprüfer antwortet nie, andere 9999 zugeordnete Resultate und Sollmanifest sind erhalten

**Warum bleibt oder endet der Zustand?** Barrier wartet bis Pflichtbeleg oder zulässige explizite Fehlerdisposition, allein ein Parent erschöpft nicht alle Slots

**Zugeordnete Residues:** [OBR015: Teilresultat mit Fehlstellen](residues.md#obr015)

**Was bleibt warum nutzbar?** Operator kann vorhandene Befunde prüfen, fachliche Vollständigkeit und Freigabe sind nicht erhalten

**Zu prüfen:** Vermisster Prüfer ist nach ursprünglichem Vertrag optional oder Teilresultat wird trotzdem als vollständig freigegeben

<a id="s225.b02"></a>
### S225.B02 — Begrenzter Parent endet unvollständig

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Vertrag erlaubt explizites unvollständiges Ende und tatsächliche Poolaufnahme bleibt begrenzt, fremde Childwirkungen werden separat reconciliert

**Warum bleibt oder endet der Zustand?** Parent gibt seinen nachgewiesenen Slot frei, fehlendes Prüfergebnis bleibt fehlend

**Zugeordnete Residues:** [OBR015: Teilresultat mit Fehlstellen](residues.md#obr015), [OBR016: Abgetrennte Restkapazität](residues.md#obr016)

**Was bleibt warum nutzbar?** Teildiagnose und nicht vom wartenden Parent belegte Restkapazität bleiben nutzbar, kein erfundener zehntausendster Befund

**Zu prüfen:** Parent behält nach terminaler Disposition seinen Slot oder fehlendes Ergebnis erscheint als erfolgreich geprüft

**Architekturfolge für diesen Stressor:** Sollmenge und fehlende Prüfer explizit halten. Zeitlich begrenzte Ablehnung oder Teildiagnose dürfen nicht unbemerkt All-of-Semantik ersetzen.

<a id="s226"></a>
## S226 — Ein Optimierer verdoppelt langsame Rechnungsaktionen spekulativ um das Tail zu verkürzen

**Ursprung:** observability; A6-Zustände: OB17, OB16, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s226.b01"></a>
### S226.B01 — Zwei spekulative Rechnungswirkungen angenommen

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Hedges nutzen unterschiedliche Identitäten oder Teilnehmer dedupliziert nicht, beide werden irreversibel akzeptiert, lokales Journal erhalten

**Warum bleibt oder endet der Zustand?** Historische Duplikation bleibt, spätere finanzielle Korrektur kann Schaden mindern aber Annahmen nicht ungeschehen machen

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Operator kann erhaltene Versuche zur Folgenbearbeitung nutzen, ursprüngliche Nichtduplikation ist verloren

**Zu prüfen:** Teilnehmerbuch zeigt nur eine Annahme der Geschäftsoperation trotz zweier Aufrufe

<a id="s226.b02"></a>
### S226.B02 — Zwei Aufrufe, eine gebundene Annahme

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide erlaubten Hedges tragen dieselbe stabile Geschäftsidentität und Teilnehmer dedupliziert atomar über ganzen Horizont

**Warum bleibt oder endet der Zustand?** Eine fachliche Annahme bleibt terminal, erneute Transportaufrufe liefern nur Status derselben Identität

**Zugeordnete Residues:** [OBR018: Teilnehmerseitiges Annahmebuch](residues.md#obr018)

**Was bleibt warum nutzbar?** Recovery kann einen Ausgang zuordnen und Teilnehmer doppelte Annahme abweisen, Transportlast ist dadurch nicht reduziert

**Zu prüfen:** Zwei Aufrufe derselben Identität erzeugen zwei Annahmen oder Teilnehmer vergisst Identität bevor Nachzügler enden

**Architekturfolge für diesen Stressor:** Effektvolle Hedges nur mit bewiesenem Teilnehmervertrag und gültiger Aktionsfreigabe betrachten. Lokales Versuchsjournal ersetzt keine atomare Annahmededuplication.

<a id="s227"></a>
## S227 — Ein Aggregator erklärt 999 von 1000 Prüfergebnissen für vollständig um schnell zu antworten

**Ursprung:** observability; A6-Zustände: OB15, OB17, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s227.b01"></a>
### S227.B01 — 999 Befunde, Pflichtvollständigkeit verloren

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Tausend Prüfer sind erforderlich, einer fehlt und vorhandene 999 Resultate sowie Sollmenge sind erhalten

**Warum bleibt oder endet der Zustand?** Lücke bleibt bis echter fehlender Befund oder ehrliche unvollständige Disposition, schneller Report schließt sie nicht

**Zugeordnete Residues:** [OBR015: Teilresultat mit Fehlstellen](residues.md#obr015)

**Was bleibt warum nutzbar?** Operator kann 999 Resultate prüfen, Aggregator darf damit weder volle Prüfung noch zulässige Aktion behaupten

**Zu prüfen:** Ursprünglicher Vertrag verlangt tatsächlich nur 999 und fehlender Prüfer hat kein Vetorecht

<a id="s227.b02"></a>
### S227.B02 — Fehlendes Veto führt zu angenommener Fehlaktion

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Tausendster Pflichtprüfer hätte vetoisiert, fälschlich vollständiger Report öffnet Dispatch und irreversible Fehlaktion wird angenommen

**Warum bleibt oder endet der Zustand?** Historische falsche Annahme bleibt auch wenn Prüfung später vervollständigt wird

**Zugeordnete Residues:** [OBR015: Teilresultat mit Fehlstellen](residues.md#obr015)

**Was bleibt warum nutzbar?** Erhaltenes Soll-/Istmanifest erlaubt Fehlerursache zu prüfen, Freigabekorrektheit der vergangenen Aktion ist verloren

**Zu prüfen:** Kein nachfolgender Effekt wurde angenommen oder fehlender Pflichtbefund hätte Aktion nicht verhindert

<a id="s227.b03"></a>
### S227.B03 — Ehrlicher ursprünglicher Schwellvertrag erfüllt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor Beginn erlaubter Vertrag verlangt genau erreichten Schwellwert, fehlende Identität wird offengelegt und hat kein Pflichtveto

**Warum bleibt oder endet der Zustand?** Erfüllung gilt nur für diesen engeren Vertrag, keine nachträgliche Umschreibung der All-of-Zusage

**Zugeordnete Residues:** [OBR015: Teilresultat mit Fehlstellen](residues.md#obr015)

**Was bleibt warum nutzbar?** Auftraggeber kann qualifiziertes Ergebnis samt Nenner nutzen ohne fehlenden Befund zu erfinden

**Zu prüfen:** Schwelle wird erst nach Ausbleiben geändert oder Verbraucher sieht eine Tausend-von-Tausend-Behauptung

**Architekturfolge für diesen Stressor:** Pflichtmenge vor Beginn binden. Schwellenergebnis ist nur bei ursprünglichem Schwellvertrag vollständig, ein fehlendes Veto bleibt echte Lücke.

<a id="s228"></a>
## S228 — Eine von hunderttausend Anfragen hängt unbegrenzt und belegt über Jahre alle Slots

**Ursprung:** observability; A6-Zustände: OB10, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s228.b01"></a>
### S228.B01 — Immer mehr Slots dauerhaft belegt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Endlicher Pool, fortgesetzte Ankünfte mit positiver Hangwahrscheinlichkeit und keine Freigabe, bestätigte Taskdaten bleiben lesbar

**Warum bleibt oder endet der Zustand?** Jeder Hang hält einen weiteren Slot, bei Ende neuer Ankünfte stoppt Wachstum aber belegte Slots bleiben

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann erhaltene Aufträge inspizieren, verfügbare Slots und Zeit bis Erschöpfung sind nicht aus Häufigkeit allein gesichert

**Zu prüfen:** Alle Hänger geben Slots innerhalb einer belegten endlichen Grenze frei

<a id="s228.b02"></a>
### S228.B02 — Nicht vom Hang belegbarer Pool bleibt nutzbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Gesamtaufnahme je Pool ist begrenzt und reale getrennte Kapazität schützt andere zugelassene Arbeit, Hänger behalten ihre belegten Slots

**Warum bleibt oder endet der Zustand?** Störpool kann voll bleiben, isolierter Rest dient weiter solange dessen eigener Bedarf passt

**Zugeordnete Residues:** [OBR016: Abgetrennte Restkapazität](residues.md#obr016)

**Was bleibt warum nutzbar?** Scheduler kann tatsächlich freie reservierte Kapazität nutzen, Timeout allein beendet Hänger oder deren Writes nicht

**Zu prüfen:** Ein Hangpool sättigt den gemeinsamen Lock und verhindert auch Arbeit der angeblich isolierten Klasse

**Architekturfolge für diesen Stressor:** Slotresidenz und belegte Isolation messen, nicht aus seltenem Prozentwert eine Lebensdauer ableiten. Reclamation erfordert echte Beendigung oder Fence.

<a id="s229"></a>
## S229 — Nur ein bestimmter Scope erlebt minutenlange fsync-Pausen während globale Mittelwerte grün sind

**Ursprung:** observability; A6-Zustände: OB09, OB03, OB13. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s229.b01"></a>
### S229.B01 — Ein Scope verpasst Fristen, Rest hat Service

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Fsync-Pausen betreffen wirklich nur isolierte Scopekapazität, ursprüngliche Frist kürzer, Zeit- und Aufnahmebelege erhalten

**Warum bleibt oder endet der Zustand?** Wiederkehrende scopebezogene Pausen erhalten Latenzproblem, globales Mittel glättet es ohne Ursache zu ändern

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012), [OBR013: Vollständiger Aufnahmenenner](residues.md#obr013), [OBR016: Abgetrennte Restkapazität](residues.md#obr016)

**Was bleibt warum nutzbar?** Andere isolierte Scopes können arbeiten und Operator sieht betroffenen Nenner, verlorene Scopefristen bleiben verloren

**Zu prüfen:** Ein globaler Writerlock überträgt denselben Stillstand auf alle behauptet isolierten Scopes

<a id="s229.b02"></a>
### S229.B02 — Globaler Writer zieht andere Scopes mit

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Fsync hält tatsächlich gemeinsamen nicht präemptierbaren Writer, alter Store lesbar

**Warum bleibt oder endet der Zustand?** Andere Writes warten bis fsync endet, Serviceanteil auf höherer Queueebene schafft keinen I/O-Fortschritt

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001), [OBR013: Vollständiger Aufnahmenenner](residues.md#obr013)

**Was bleibt warum nutzbar?** Reads und aufgenommene offene Arbeit bleiben inspizierbar, keine garantierte Schreibrestkapazität

**Zu prüfen:** Neue Writes anderer Scopes committen während derselbe globale Lock unverändert gehalten wird

**Architekturfolge für diesen Stressor:** Scopebezogene Aufnahme- und Commitzeiten mit tatsächlicher I/O-Topologie verbinden. Faire Queues können einen ununterbrechbaren globalen fsync nicht isolieren.

<a id="s230"></a>
## S230 — Eine zufällige Speicherbereinigung trifft immer dieselbe Geschäftsdeadline

**Ursprung:** observability; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s230.b01"></a>
### S230.B01 — Periodische Allokation bindet Pause an Geschäftstermin

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Gering eingreifende Messung zeigt periodische Allokationsschwelle und gleichbleibende Deadlinephase, nicht echten unabhängigen Zufall

**Warum bleibt oder endet der Zustand?** Wiederkehrende Last triggert Sammlung, feste Phase legt Pause immer auf Termin, Verschieben der Phase kann Kollision beenden

**Zugeordnete Residues:** [OBR011: Vergleichbare Timing-Evidenz](residues.md#obr011), [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Analyst kann Kollision und unveränderten Termin anhand Timingdossier prüfen, vergangene Frist ist nicht gerettet

**Zu prüfen:** Phase verschieben bei unveränderter Allokation ändert Kollisionen nicht oder Trigger ist tatsächlich unabhängig

<a id="s230.b02"></a>
### S230.B02 — Zufall oder selektive Erinnerung bleibt offen

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Triggerverteilung Stichprobengröße und Auswahl der berichteten Deadlines fehlen

**Warum bleibt oder endet der Zustand?** Behauptetes immer trennt Zufall Alias und Beobachterselektion nicht, kein eindeutiges Regime

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für konkrete kausale Timingdiagnose ist kein ausreichend spezifiziertes Dossier belegt

**Zu prüfen:** Vorregistrierte lange Exposition samt Zufallsbaseline weist stabilen Phaseneffekt nach

**Architekturfolge für diesen Stressor:** Collectortrigger Deadlinephase Jitter Exposition und Auswahlmodus gemeinsam erfassen. Wiederholtes Zusammentreffen allein ist keine Lock-in-Dynamik.

<a id="s231"></a>
## S231 — Die Antwort auf einen Versand kommt 90 Tage nach Ablauf der Idempotenzhistorie

**Ursprung:** observability; A6-Zustände: OB28, OB16, OB17, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s231.b01"></a>
### S231.B01 — Letzte notwendige Zuordnung erloschen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Lokale Historie gelöscht und Inventar schließt alle rechtmäßig zugänglichen Teilnehmerzuordnungen und Kopien im benötigten Horizont aus

**Warum bleibt oder endet der Zustand?** Für dieses alte Ergebnis bleibt Klassifikation unreproduzierbar, neue Annahmeevidenz würde Inventarprämisse widerlegen

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Zuordnung dieses 90 Tage verspäteten Ergebnisses ist verloren, kein Verlust des restlichen Stores oder automatische Zweitwirkung behauptet

**Zu prüfen:** Eine rechtmäßig erhaltene stabile Teilnehmeridentität ordnet Antwort eindeutig zu

<a id="s231.b02"></a>
### S231.B02 — Teilnehmer kennt Identität noch

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Lokale Historie abgelaufen, aber derselbe Teilnehmer führt tatsächlich noch autorisiert abfragbares Annahmebuch über den ganzen Horizont

**Warum bleibt oder endet der Zustand?** Passende Abfrage beendet genau die Ausgangsungewissheit, lokale Löschung wird nicht als nie versucht gelesen

**Zugeordnete Residues:** [OBR018: Teilnehmerseitiges Annahmebuch](residues.md#obr018)

**Was bleibt warum nutzbar?** Recovery kann diesen Ausgang und eventuelle Retries derselben Identität unterscheiden, keine externe Kopie ohne Voraussetzung ergänzt

**Zu prüfen:** Teilnehmer kann wegen eigener Retention oder neuer Retryidentität den Zusammenhang nicht eindeutig liefern

<a id="s231.b03"></a>
### S231.B03 — Nur lokale Sicht fehlt, globale Lage unbekannt

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Lokale Historie weg, Inventar externer Retention und eventueller bereits erfolgter Retries ist unbekannt

**Warum bleibt oder endet der Zustand?** Bis Inventar oder rechtmäßige Evidenz vorliegt bleiben Ausgang und Duplikatzahl offen

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Kein nutzbarer Korrelationsgegenstand ist aktuell belegt, aus fehlender lokaler Historie darf weder Totalverlust noch zweite Annahme folgen

**Zu prüfen:** Autorisierte Abfrage zeigt entweder stabile Zuordnung oder ein geschlossenes Inventar belegt endgültigen Verlust

**Architekturfolge für diesen Stressor:** Lokale und externe Retention samt spätester Antwort gemeinsam festlegen. Späte Antwort allein dupliziert nichts; ohne lawful Korrelation keine rekonstruierte Gewissheit.

<a id="s232"></a>
## S232 — Ein offline gepufferter Auftrag wird 2046 ausgeführt nachdem Empfänger und Firma gewechselt haben

**Ursprung:** observability; A6-Zustände: OB17, OB07, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s232.b01"></a>
### S232.B01 — 2046 angenommene veraltete Aktion

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Empfänger oder Firma hat sich geändert und Teilnehmer akzeptiert alte nicht mehr gültige Absicht, Versuchsjournal erhalten

**Warum bleibt oder endet der Zustand?** Vergangene ungewollte Wirkung bleibt nach Stop wahr, statischer alter Empfänger allein beweist alte Erlaubnis nicht

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Recovery kann dokumentierten Versuch zur Folgenklärung nutzen, Gültigkeit dieser angenommenen Aktion ist nicht erhalten

**Zu prüfen:** Teilnehmer lehnt vor Annahme ab oder es liegt eine tatsächlich passende aktuelle Erlaubnis vor

<a id="s232.b02"></a>
### S232.B02 — Alter Puffer bleibt prüfbarer Halt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Gültigkeitsgate kontrolliert letzten Dispatch und verlangt neue passende Autorität sowie gebundene Empfängeridentität bei relevanter Änderung

**Warum bleibt oder endet der Zustand?** Alte Bytes bleiben Absicht bis berechtigte neue Entscheidung oder expliziter Ablauf

**Zugeordnete Residues:** [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007), [OBR035: Aktionsgebundene Freigabeevidenz](residues.md#obr035)

**Was bleibt warum nutzbar?** Operator kann Absicht prüfen und Gate falsche neue Identität ablehnen, bereits autonom beim Teilnehmer laufender Puffer wäre außerhalb Grenze

**Zu prüfen:** Alter Offlineeintrag wird trotz geänderter gebundener Identität ohne neue Freigabe angenommen

**Architekturfolge für diesen Stressor:** Offlinepuffer darf nicht neue Gegenwartserlaubnis aus alten Bytes ableiten. Empfängerbindung und Unternehmensnachfolge getrennt vom Erstellungsdatum prüfen.

<a id="s233"></a>
## S233 — Ein Ergebnis trifft nach legaler Löschung seiner Zuordnung ein und enthält sensible Daten

**Ursprung:** observability; A6-Zustände: OB28, OB16, OB21, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s233.b01"></a>
### S233.B01 — Korrelation nach endgültiger Löschung verloren

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Erforderliche Zuordnung wurde rechtmäßig überall im zulässigen Inventar gelöscht, keine erlaubte Rekonstruktion vorhanden

**Warum bleibt oder endet der Zustand?** Für dieses Ergebnis bleibt richtige Zuordnung unmöglich, Datenschutzgewinn der Löschung bleibt davon getrennt

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Fähigkeit zur Zuordnung ist verloren, sensible Bytes selbst sind kein nutzbares Zustellrecht und keine imaginäre Quarantäne wird angenommen

**Zu prüfen:** Eine rechtmäßig zugängliche unabhängige Zuordnung löst den Rückläufer ohne verbotene Wiederherstellung auf

<a id="s233.b02"></a>
### S233.B02 — Unzugeordneten Inhalt vor Weitergabe verworfen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Entwurfsgrenze erlaubt und erzwingt frühe begrenzte Ablehnung ohne sensible Logs oder Speicherung, Zuordnung bleibt gelöscht

**Warum bleibt oder endet der Zustand?** Dieser Rückläufer endet ohne Routing, spätere Rückläufer benötigen dieselbe eigenständige Prüfung

**Zugeordnete Residues:** [OBR042: Zuordnungsfreie Rücklaufablehnung](residues.md#obr042)

**Was bleibt warum nutzbar?** Kleine zulässige Protokollverarbeitung und klare Ablehnung bleiben nutzbar, weder Inhalt noch Identitätszuordnung werden als erhalten ausgegeben

**Zu prüfen:** Ablehnung puffert unbegrenzt oder schreibt sensible Payload dennoch in Log beziehungsweise fremden Scope

<a id="s233.b03"></a>
### S233.B03 — Geratene Zuordnung legt Daten offen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Handler routet oder loggt Rückläufer zu unberechtigtem Leser und dieser erhält Klartext

**Warum bleibt oder endet der Zustand?** Offengelegte Fakten lassen sich nicht zurückholen, Stop verhindert höchstens weitere Offenlegung

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Geheimhaltung dieser gelesenen Payload bleibt kein Residue, andere nicht offengelegte Daten sind nicht automatisch verloren

**Zu prüfen:** Kein unberechtigter Leser erhält tatsächlich Inhalte oder der Handler verwirft vor Offenlegung

**Architekturfolge für diesen Stressor:** Nach legaler Löschung keine Zuordnung heimlich retten. Unassoziierte sensible Rückläufer müssen vor Log Routing und dauerhafter Aufnahme mit expliziter Rechtsgrenze behandelt werden.

<a id="s234"></a>
## S234 — Cancel erreicht den Teilnehmer eine Mikrosekunde nach irreversibler Annahme

**Ursprung:** observability; A6-Zustände: OB17, OB16, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s234.b01"></a>
### S234.B01 — Cancel nach tatsächlicher Annahme

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Teilnehmer akzeptiert irreversible Aktion eine Mikrosekunde vorher und führt autorisiert lesbares Annahmebuch

**Warum bleibt oder endet der Zustand?** Vergangene Aktion kann nicht ungeschehen werden, spätere Cancelwirkung beendet nur weitere Arbeit

**Zugeordnete Residues:** [OBR018: Teilnehmerseitiges Annahmebuch](residues.md#obr018)

**Was bleibt warum nutzbar?** Operator kann echte Annahmereihenfolge und Folgen klären, Nichtausführung ist für diese Aktion verloren

**Zu prüfen:** Teilnehmerledger belegt Cancel-Fence vor tatsächlichem irreversiblen Annahmepunkt

<a id="s234.b02"></a>
### S234.B02 — Lokaler Cancel ohne bekannte externe Ordnung

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur lokale Cancelanfrage und Versuchsjournal sind erhalten, externe Antwort oder Reihenfolgenachweis fehlt

**Warum bleibt oder endet der Zustand?** Warten endet erst durch passende Teilnehmerbelege, Timeout ist weder Erfolg noch Rollback

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Recovery kann Wiederholung zurückhalten und Unsicherheit zeigen, lokale Anfrage beweist externe Annahme oder Nichtannahme nicht

**Zu prüfen:** Passende autoritative externe Reihenfolge liegt bereits vor und löst den konkreten Ausgang auf

<a id="s234.b03"></a>
### S234.B03 — Cancel gewinnt am Wirkungspunkt als Gegenbedingung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Anderer Timingzweig: Teilnehmer wendet kontrolliertes Cancel-Fence vor Annahme an und belegt keine frühere Annahme

**Warum bleibt oder endet der Zustand?** Für genau diese Operation endet weitere Annahmefähigkeit, vergangene andere Aktionen bleiben getrennt

**Zugeordnete Residues:** [OBR018: Teilnehmerseitiges Annahmebuch](residues.md#obr018)

**Was bleibt warum nutzbar?** Operator kann echte Abbruchwirkung von bloßer Kanalannahme unterscheiden, dieser Zweig gilt nicht bei vorgegebener umgekehrter Mikrosekundenordnung

**Zu prüfen:** Spätere Nachricht derselben Operation wird trotz bestätigtem vorgelagertem Cancel-Fence noch angenommen

**Architekturfolge für diesen Stressor:** Cancel-Anfrage Cancelannahme und externe irreversible Annahme getrennt belegen. Mikrosekundenordnung am Teilnehmer zählt, lokaler Timestamp ist kein Fence.

<a id="s235"></a>
## S235 — Der Parent stirbt nach Timeout aber ein Enkelprozess schreibt zwei Tage weiter

**Ursprung:** observability; A6-Zustände: OB18, OB32, OB17, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s235.b01"></a>
### S235.B01 — Enkel schreibt nach Parenttod weiter

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Enkel lebt mit eigener Schreibautorität zwei Tage weiter, Lease bleibt im betrachteten Pfad gehalten

**Warum bleibt oder endet der Zustand?** Endliche Enkelarbeit braucht keine Rückkopplung, Ende erst mit tatsächlichem Exit oder wirksamem Autoritätsentzug

**Zugeordnete Residues:** [OBR021: Belegter unklarer Besitz](residues.md#obr021)

**Was bleibt warum nutzbar?** Scheduler kann besetzte Ressource vor Wiedervergabe halten, das stoppt den Enkel selbst nicht und bestätigt kein Taskergebnis

**Zu prüfen:** Lease wird trotz behaupteter Haltegrenze wiedervergeben oder Enkel konnte seit Parentende überhaupt nicht schreiben

<a id="s235.b02"></a>
### S235.B02 — Wiedervergabe erzeugt zwei Writer

**Art:** konflikt. **Residue-Status:** teilweise.

**Voraussetzungen:** TTL oder presumed-gone-Entscheidung führt tatsächlich zu Ersatzstart während Enkel weiterhin schreiben darf, Originaljournal lesbar

**Warum bleibt oder endet der Zustand?** Zwei Writer können bis Entzug oder Ende kollidieren, bloßer Ersatzdatensatz ohne Start reicht nicht

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Früher bestätigte Daten bleiben Inspektionsgrundlage soweit intakt, Single-Writer-Eigenschaft ist nicht erhalten

**Zu prüfen:** Kein Ersatzprozess startet oder alter Enkel verliert vor Start nachweislich Schreibrechte

**Nachtrag des Koordinators:** [A7S04](../review-dispositions.md#a7s04); ursprüngliche Abgabe unverändert.

<a id="s235.b03"></a>
### S235.B03 — Alle Nachkommen wirksam eingefasst

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Vollständige Autoritätsinventur und Fence am tatsächlichen Zugriffspunkt entziehen auch abgetrenntem Enkel Rechte vor Wiedervergabe

**Warum bleibt oder endet der Zustand?** Alte Prozesse können weiter existieren aber keine neuen autorisierten Writes ausführen, vergangene Writes bleiben abzugleichen

**Zugeordnete Residues:** [OBR020: Entzogene Schreibautorität](residues.md#obr020)

**Was bleibt warum nutzbar?** Neuer Besitzer kann frei von alter Schreibautorität arbeiten, Parent-PID oder verstrichene Zeit allein genügen nicht

**Zu prüfen:** Abgetrennter Enkel schreibt nach bestätigter Fencegrenze weiterhin über eigene Credentials oder OS-Bypass

**Architekturfolge für diesen Stressor:** Parentende nicht als gesamte Autoritätsbeendigung führen. Leasewiedervergabe nur bei geprüftem Eigentums- und Fencevertrag, vergangene Enkelwrites separat abgleichen.

<a id="s236"></a>
## S236 — Ein Cancel steckt hinter Gigabytes stdout im selben Kanal fest

**Ursprung:** observability; A6-Zustände: OB35, OB01, OB17. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s236.b01"></a>
### S236.B01 — Cancel wartet hinter endlichem stdout

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Gemeinsamer Leser oder Scheduler verarbeitet Gigabytes vor Cancel, noch keine unabhängige Controlkapazität, Versuchsjournal bleibt lesbar

**Warum bleibt oder endet der Zustand?** Bei endlicher Ausgabe kann Kanal später drainieren, bis dahin kann Teilnehmer weiterarbeiten oder irreversibel annehmen

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Recovery kann bekannte Versuche und unbestätigten Cancelstatus lesen, zeitgerechter Abbruch ist nicht erhalten

**Zu prüfen:** Cancel wird bei unverändert gesättigtem gemeinsamem Pfad innerhalb zugesagter Grenze tatsächlich verarbeitet

<a id="s236.b02"></a>
### S236.B02 — Control trotz Datenrückstau bedienbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorgeschlagene Controlaufnahme Timer und Supervisor haben eigene begrenzte Verarbeitung, vor Wirkungsannahme ist Stop noch möglich

**Warum bleibt oder endet der Zustand?** Cancel kann unabhängig fortschreiten, tatsächliches Ende braucht eigene Bestätigung am Wirkungspunkt

**Zugeordnete Residues:** [OBR019: Bedienbarer Kontrollpfad](residues.md#obr019)

**Was bleibt warum nutzbar?** Operator kann Anfrage wirksam weiterleiten und neue Aufnahme sperren ohne stdout abzuwarten, bereits angenommene Aktion bleibt irreversibel

**Zu prüfen:** Datenflut verbraucht den angeblich reservierten Eventloopturn oder Controlannahme wird ohne Wirkung als fertig ausgegeben

**Architekturfolge für diesen Stressor:** Control braucht reservierte Verarbeitung durch alle geteilten Leser und Scheduler, nicht nur zwei Pipe-Richtungen. Annahmefrist ersetzt keine externe Wirkungsfrist.

<a id="s237"></a>
## S237 — CLI Kernel Plugin und SDK wiederholen jeweils viermal denselben Fehler

**Ursprung:** observability; A6-Zustände: OB12, OB11, OB17. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s237.b01"></a>
### S237.B01 — Endlicher multiplizierter Retrybaum

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Vier Schichten wiederholen unabhängig aber endlich, keine frischen Wurzeln, Journal der logischen Aktion bleibt erhalten

**Warum bleibt oder endet der Zustand?** Endlicher Versuchsvorrat kann nach Fehlerende drainieren, Multiplikation allein ist kein Attraktor

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Operator kann logische Wirkung als ungewiss halten, viele Transportversuche beweisen weder mehrere Annahmen noch vorhandene Dienstkapazität

**Zu prüfen:** Nach vollständigem Verbrauch entstehen ohne deklarierte neue Autorität weitere Versuche

<a id="s237.b02"></a>
### S237.B02 — Wiederholungen erhalten Überlast nach Fehlerende

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Deklarierter positiver Grundbedarf liegt unter gesunder Kapazität, Timeoutfolgen erneuern Retryarbeit über endliche Teilbudgets hinaus, Rückstau senkt Erfolgsrate und Journal bleibt lesbar

**Warum bleibt oder endet der Zustand?** Lange Queue erzeugt Timeouts, diese neue Versuche, diese längere Queue und weniger nützlichen Dienst auch nach ursprünglichem Fehlerende

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Lokale Wirkungsungewissheit bleibt noch inspizierbar, rechtzeitiger Durchsatz und externe Nichtduplikation sind nicht bewiesen

**Zu prüfen:** Nach Fehlerende bei gleichem Grundbedarf gleicher Policy und variiertem Rückstau drainiert jede behauptete Überlastlage, Null-Neuzugänge separat prüfen

<a id="s237.b03"></a>
### S237.B03 — Ende-zu-Ende-Budget bleibt zählbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle Retryschichten konsumieren dieselbe stabile Operationsidentität und begrenzte Gesamtzulassung, verdeckte SDK-Versuche ausgeschlossen

**Warum bleibt oder endet der Zustand?** Budgetende hält weitere Versuche bis neue explizite Erlaubnis, keine heimliche Erneuerung durch Layerwechsel

**Zugeordnete Residues:** [OBR022: Gemeinsames Versuchsbudget](residues.md#obr022)

**Was bleibt warum nutzbar?** Dispatcher kann erlaubte Restversuche nutzen und Gesamtlast begrenzen, dieses Budget verhindert allein keine zwei externen Annahmen

**Zu prüfen:** Leafzahl überschreitet gemeinsame Grenze obwohl keine neue Erlaubnis erteilt wurde

**Architekturfolge für diesen Stressor:** Gemeinsames endliches Versuchsbudget von Teilnehmerdedup trennen. Vier Gesamtversuche je Schicht ergeben höchstens 256 Blätter, vier Wiederholungen fünf Versuche und höchstens 625.

<a id="s238"></a>
## S238 — Eine Million Clients versuchen nach identischer Pause exakt gleichzeitig erneut

**Ursprung:** observability; A6-Zustände: OB12, OB11, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s238.b01"></a>
### S238.B01 — Einmaliger synchroner Millionenstoß

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Endliche Clientmenge wiederholt genau eine begrenzte Runde, reale Restkapazität und Gesamtaufnahme sind begrenzt

**Warum bleibt oder endet der Zustand?** Nach Ablehnung oder Bearbeitung endet Kohorte, neue Peaks brauchen zusätzliche Retryregel

**Zugeordnete Residues:** [OBR016: Abgetrennte Restkapazität](residues.md#obr016), [OBR022: Gemeinsames Versuchsbudget](residues.md#obr022)

**Was bleibt warum nutzbar?** Zugelassene Arbeit kann reservierte Kapazität nutzen und Restversuche bleiben zählbar, nicht alle Millionen Anfragen müssen erfolgreich werden

**Zu prüfen:** Ohne neue Autorität entstehen nach Abschluss der Kohorte weitere synchrone Wellen

<a id="s238.b02"></a>
### S238.B02 — Gleiche Timer koppeln neue Überlastwellen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Timeouts erzeugen erneut synchronisierte Versuche und deklarierter Grundbedarf allein wäre tragbar, dauerhafte Absichten bleiben lesbar

**Warum bleibt oder endet der Zustand?** Welle überfüllt Dienst, Fehler setzen gleiche Timer, nächste Welle erhält Fehler auch bei repariertem Provider

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann erhaltene Absichten inspizieren, zeitgerechte Bedienung oder Budgeterhalt sind nicht gesichert

**Zu prüfen:** Nach Fehlerende bei festem Grundbedarf und gleicher Policy erzeugt variierter Rückstau keine erneuten Peaks, danach separat Neuzugänge stoppen

**Architekturfolge für diesen Stressor:** Admission und Retrybudget vor Jitter priorisieren. Timerstreuung verteilt Last, schafft aber keine Kapazität oder zulässige unbegrenzte Retryautorität.

<a id="s239"></a>
## S239 — Nach Quotenfehler retryt ein Plugin aggressiver und verbraucht das letzte Budget

**Ursprung:** observability; A6-Zustände: OB11, OB07. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s239.b01"></a>
### S239.B01 — Retries fressen endlichen Restbetrag

**Art:** eskalation. **Residue-Status:** keines.

**Voraussetzungen:** Auch Quotenfehler kosten knappe Mittel, Plugin reagiert mit mehr Calls und kein vorab wirksames Ausgabenlimit

**Warum bleibt oder endet der Zustand?** Quotenfehler erhöhen Retryrate, Calls verbrauchen Rest und erzeugen mehr Fehler, ohne Budgetreset endet Wachstum spätestens bei harter Sperre

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für den bereits verbrauchten Restbetrag ist keine Spendefähigkeit erhalten, gratis Fehler würden diesen Mechanismus nicht tragen

**Zu prüfen:** Fehlgeschlagene Calls sind kostenlos oder hartes Gate stoppt vor zusätzlicher Belastung

<a id="s239.b02"></a>
### S239.B02 — Reservierter Rest bleibt gebundene Erlaubnis

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle kostenpflichtigen Versuche verbrauchen vorher bekanntes reserviertes Budget, Quotenfehler öffnen keine neuen Mittel

**Warum bleibt oder endet der Zustand?** Gate hält nach Budgetende bis neue berechtigte Ausgabenentscheidung, Providerreset ist externer Anlass

**Zugeordnete Residues:** [OBR023: Begrenzte Ausgabenerlaubnis](residues.md#obr023)

**Was bleibt warum nutzbar?** Dispatcher kann noch reservierte zulässige Versuche planen und weitere Ausgabe verweigern, vorhandene Quote allein garantiert keinen Providererfolg

**Zu prüfen:** Nachbelastung oder verborgene SDK-Calls überschreiten reservierten Betrag ohne neue Freigabe

**Architekturfolge für diesen Stressor:** Quotenantwort und tatsächlich belastbare Kosten unterscheiden. Hart erschöpftes Budget ist Halt, nicht unbegrenzt weiterlaufender Überlastattraktor.

<a id="s240"></a>
## S240 — Nach Stromrückkehr laden alle Agenten gleichzeitig ihre riesigen Modelle

**Ursprung:** observability; A6-Zustände: OB12, OB26, OB11, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s240.b01"></a>
### S240.B01 — Begrenzte kalte Kohorte wird warm

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Modelle werden über durchgesetzte Ladeplätze gestartet und Gesamtspitzenbedarf passt, erfolgreiche Warmzustände werden nicht sofort verdrängt

**Warum bleibt oder endet der Zustand?** Jedes Modell lädt endlich einmal, Startstoß endet ohne neue Ladeschleife

**Zugeordnete Residues:** [OBR024: Kaltstartfähige Zulassung](residues.md#obr024)

**Was bleibt warum nutzbar?** Operator kann schrittweise nutzbare warme Agenten erhalten statt Gleichzeitigkeit zu versprechen

**Zu prüfen:** Erfolgreiches Laden wird vor Nutzung verdrängt oder tatsächliche Spitzenbelegung überschreitet physische Grenze

<a id="s240.b02"></a>
### S240.B02 — Eviction und Neustart reproduzieren Ladelast

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Lastspitze erzeugt Speicherdruck, Loader sterben oder Modelle werden vor Nutzung evicted, Neustartbudget erneuert sich, Intent auf Disk erhalten

**Warum bleibt oder endet der Zustand?** Erneutes Laden beansprucht Speicher, Druck zerstört Warmfortschritt, fehlende Readiness löst nächste Ladung aus

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Bestätigte Aufträge bleiben später lesbar, benutzbare warme Kapazität ist nicht gesichert

**Zu prüfen:** Bei Fehlerende und festem Start-/Grundbedarf stabilisiert jeder getestete Rückstau ohne erneute Loader, Null-Neustarts wäre gesonderter Policywechsel

**Architekturfolge für diesen Stressor:** Kaltstart-Spitzenbedarf und Ladezulassung getrennt vom Warmdurchsatz behandeln. Endliche Kohorte oder harter Startfehler sind keine wiedererzeugte Überlast.

<a id="s241"></a>
## S241 — Ein leerer Cache macht eine bisher optionale Suche zum Kapazitätsengpass

**Ursprung:** observability; A6-Zustände: OB26, OB13, OB11, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s241.b01"></a>
### S241.B01 — Leerer Cache macht Suche zum echten Pflicht-Halt

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Kern wartet synchron auf benötigte Suche, kalt benötigter Dienst liefert nicht rechtzeitig, Intent und bestätigte Daten bleiben lesbar

**Warum bleibt oder endet der Zustand?** Warten endet erst bei erfolgreichem Warming oder expliziter Fehlerdisposition, Cacheverlust allein ist keine Schleife

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann Absicht und Bestand weiter prüfen, fachlich richtiges Ergebnis ohne notwendige Suchinformation ist nicht erhalten

**Zu prüfen:** Kern liefert bei dauerhaft fehlender Suche nach ursprünglichem Vertrag akzeptables Ergebnis

<a id="s241.b02"></a>
### S241.B02 — Erlaubtes Grundresultat ohne Anreicherung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ursprünglicher Vertrag erlaubt ausdrücklich Suchauslassung und Kern benutzt separaten begrenzten Pfad

**Warum bleibt oder endet der Zustand?** Kern endet mit markierter fehlender Anreicherung auch solange Suche kalt oder ausgefallen bleibt

**Zugeordnete Residues:** [OBR025: Kernresultat ohne optionale Suche](residues.md#obr025)

**Was bleibt warum nutzbar?** Auftraggeber kann wirklich nutzbares eingeschränktes Resultat erhalten, keine vollständige Suchqualität behaupten

**Zu prüfen:** Ausgelassene Suche enthält zwingende Autorisierung oder Fakten ohne die Grundresultat unzulässig ist

**Architekturfolge für diesen Stressor:** Optionalität fachlich und im tatsächlichen kritischen Pfad nachweisen. Keine richtige Grundantwort ohne Suche erfinden wenn Suche Pflichtinformation liefert.

<a id="s242"></a>
## S242 — Kalte DNS- und TLS-Aufbauten verbrauchen die gesamte Erstaufruffrist

**Ursprung:** observability; A6-Zustände: OB26, OB09, OB01, OB11. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s242.b01"></a>
### S242.B01 — Erstaufruffrist verbraucht, späterer Warmweg möglich

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Kalter Aufbau verbraucht reale Endfrist, erfolgreich etablierte Verbindung darf danach für künftige gültige Aufträge bleiben, Timingbelege erhalten

**Warum bleibt oder endet der Zustand?** Erster Termin ist verloren, erhaltene Verbindung kann spätere Kaltkosten mindern aber keine vergangene Zusage heilen

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator kann Setupverlust messen und spätere Restbudgets korrekt beginnen, OBR012 behauptet selbst keine bestehende Connectionpool-Fähigkeit

**Zu prüfen:** Kaltes Ende-zu-Ende-Ergebnis lag bereits innerhalb ursprünglicher Frist oder Trace schließt DNS/TLS aus

<a id="s242.b02"></a>
### S242.B02 — Timeout vernichtet jeden Warmfortschritt

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Jeder Timeout bricht Setup ab, erneuerte Versuche starten kalt und konkurrierende Setups verlängern Aufbau weiter, Zeitbelege und Intent bleiben erhalten

**Warum bleibt oder endet der Zustand?** Abbruch erzeugt neuen Kaltversuch, Parallelversuche verringern Aufbaukapazität, erneute Timeouts verhindern Warmzustand

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012), [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Frist- und Bestandsdiagnose bleiben möglich, erfolgreiche Anwendungsleistung ist nicht erhalten

**Zu prüfen:** Bei festem Grundbedarf und gleicher Retrypolicy nach Fehlerende bleibt Warmfortschritt erhalten und Rückstau drainiert statt Setup neu zu erzeugen

**Architekturfolge für diesen Stressor:** DNS TLS und Queue unter denselben ursprünglichen Zeitvertrag nehmen. Erwärmung und fachliche Ausführung nicht durch blindes Budgeterneuern verwechseln.

<a id="s243"></a>
## S243 — Ein 8-Terabyte-Export steht vor allen kleinen Kontrollabfragen

**Ursprung:** observability; A6-Zustände: OB13, OB09, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s243.b01"></a>
### S243.B01 — Endlicher Export blockiert gemeinsame Schlange

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** 8 TB stehen nicht präemptierbar vor Kontrollen, positiver Durchsatz und keine weiteren Bulkankünfte, Bestand und ursprüngliche Kontrollzeitverträge sind lesbar

**Warum bleibt oder endet der Zustand?** Nach endlichem Export kann Schlange drainieren, Kontrollfristen können vorher historisch verloren sein

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001), [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator kann erhaltenen Bestand und verlorene Zeitverträge später prüfen, aktuelle Kontrollbedienung nicht garantiert

**Zu prüfen:** Kontrollen werden während desselben nicht präemptierten Exports innerhalb zugesagter Frist verarbeitet

<a id="s243.b02"></a>
### S243.B02 — Reservierter Anteil bedient kleine Kontrollen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Engpass erlaubt tatsächliche Unterbrechung oder getrennte Bedienung und garantierter Anteil deckt deklarierte Kontrolllast

**Warum bleibt oder endet der Zustand?** Export kann fortlaufen während Kontrollqueue positiven Dienst erhält, unteilbarer globaler I/O-Stall würde Grenze brechen

**Zugeordnete Residues:** [OBR026: Garantierter begrenzter Dienstanteil](residues.md#obr026), [OBR019: Bedienbarer Kontrollpfad](residues.md#obr019)

**Was bleibt warum nutzbar?** Operator kann begrenzte Kontrollen nutzen ohne ganzen Export abzuwarten, beliebige kurze Fristen folgen nicht allein aus positivem Anteil

**Zu prüfen:** Kleine Stopanfrage bleibt bei vollem Export trotz zugesagter reservierter Kapazität unbedient

**Architekturfolge für diesen Stressor:** Bulktransfer von kleinen Kontrollen über den realen Engpass trennen. Streaming ohne Unterbrechbarkeit garantiert keine Bedienung.

<a id="s244"></a>
## S244 — Ständig eintreffende Notfalltasks verdrängen normale Aufgaben über Monate

**Ursprung:** observability; A6-Zustände: OB14, OB09, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s244.b01"></a>
### S244.B01 — Dauernde Notfälle lassen Normales warten

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Strikter Vorrang und fortgesetzte Notfalllast verbrauchen ganze Kapazität, normale Absichten und ursprüngliche Zeitverträge bleiben lesbar

**Warum bleibt oder endet der Zustand?** Solange Notfälle ankommen bleibt normales Warten, nach deren Ende kann endlicher Rest drainieren

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001), [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator kann Normalbestand und verpasste Fristen prüfen, rechtzeitige normale Ausführung ist nicht erhalten

**Zu prüfen:** Normale Tasks erhalten bei dauerhaft voller Notfallqueue nachweislich positiven Service

<a id="s244.b02"></a>
### S244.B02 — Normale Arbeit behält zugesagten Anteil

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorgeschlagener Scheduler erzwingt echten Mindestanteil über teilbare Engpässe, normale eigene Last passt

**Warum bleibt oder endet der Zustand?** Notfallqueue kann voll bleiben ohne normalen Durchsatz auf null zu drücken

**Zugeordnete Residues:** [OBR026: Garantierter begrenzter Dienstanteil](residues.md#obr026)

**Was bleibt warum nutzbar?** Normale Auftraggeber können begrenzten Service nutzen, alle Notfälle sofort und alle normalen Deadlines zugleich sind nicht versprochen

**Zu prüfen:** Gemeinsamer nicht präemptierbarer Engpass nimmt trotz Quote jeden normalen Service weg

**Architekturfolge für diesen Stressor:** Notfallvorrang als explizite begrenzte Ressourcenzusage behandeln. Politisch gewollte Dauerpriorität ist äußere Last, kein selbsttragender Attraktor.

<a id="s245"></a>
## S245 — Unklare Agenten halten alle Workspace-Reservierungen und der Operator fordert TTL-Freigabe

**Ursprung:** observability; A6-Zustände: OB06, OB32, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s245.b01"></a>
### S245.B01 — Disconnected-Lease hält konkrete Reservierung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Restore- beziehungsweise disconnected-Pfad hält Lease und alle hier beteiligten Wiedervergabepfade respektieren ihn, Besitzerstatus unbekannt

**Warum bleibt oder endet der Zustand?** Fehlende Liveness hält diese Reservierung bis korrelierter Ownerbeleg oder nachgewiesener Autoritätsentzug

**Zugeordnete Residues:** [OBR021: Belegter unklarer Besitz](residues.md#obr021)

**Was bleibt warum nutzbar?** Scheduler kann genau den Workspace gegen zweite Zuteilung halten und Operator Belegbedarf lesen, Taskfreigabe und remote Ausgang bleiben separat

**Zu prüfen:** Ein anderer beteiligter Pfad vergibt diese Ressource auf bloße vermutete Abwesenheit neu

<a id="s245.b02"></a>
### S245.B02 — TTL oder presumed-gone mit tatsächlichem Ersatzstart

**Art:** konflikt. **Residue-Status:** teilweise.

**Voraussetzungen:** Unbelegte Abwesenheit wird als Freigabe benutzt, Ersatz startet wirklich und alter Besitzer kann noch schreiben, frühere Daten lesbar

**Warum bleibt oder endet der Zustand?** Überlappende Schreibautorität dauert bis Ende oder Fence, TTL selbst beendet sie nicht

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Früher bestätigter Bestand bleibt soweit intakt prüfbar, Exklusivität ist verloren, kein empirischer Vorfall aus Helpercode abgeleitet

**Zu prüfen:** Alte Autorität war vor Ersatzstart wirksam entzogen oder nur Ersatzdatensatz ohne Start existiert

**Nachtrag des Koordinators:** [A7S04](../review-dispositions.md#a7s04); ursprüngliche Abgabe unverändert.

<a id="s245.b03"></a>
### S245.B03 — Belegte alte Autorität beendet

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Tatsächlicher Ressourcenfence schließt alle alten Writes aus oder vollständig bestätigte Unbenutzbarkeit liegt vor, erst dann Leasefreigabe

**Warum bleibt oder endet der Zustand?** Ehemaliger Besitzer kann neue Annahmen nicht mehr beeinflussen, alte Effekte bleiben separat zu klären

**Zugeordnete Residues:** [OBR020: Entzogene Schreibautorität](residues.md#obr020)

**Was bleibt warum nutzbar?** Neuer Besitzer kann Exklusivität nutzen statt Zeitablauf als Todesbeweis, Remoteaktionsstatus wird nicht mit freigegeben

**Zu prüfen:** Alter Worker schreibt nach Fence oder stop-Voraussetzung wurde lediglich aus verstrichener Zeit angenommen

**Architekturfolge für diesen Stressor:** restore::reconcile und reconnect_after_herdr_or_machine_restart unterschiedlich ausweisen. Presumed-gone-Replacement ist weder positiver Todesbeweis noch schon gestarteter zweiter Writer.

<a id="s246"></a>
## S246 — Zehn Millionen registrierte Namen passen in den Store aber nicht in jede Kontextnachricht

**Ursprung:** observability; A6-Zustände: OB27, OB15, OB25, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s246.b01"></a>
### S246.B01 — Volltextauswahl passt nicht in Modellfenster

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Caller expandiert zehn Millionen Namen und Consumer schneidet oder verweigert, Registry selbst bleibt lesbar

**Warum bleibt oder endet der Zustand?** Nachrichtenfehler dauert mit übergroßer Auswahl, Registrykapazität heilt Kontextlimit nicht

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann intakten gespeicherten Bestand lesen, das Modell besitzt keine vollständige Namenkenntnis

**Zu prüfen:** Tatsächlich kompilierte Nachricht enthält gar nicht die Registryexpansion und passt vollständig

<a id="s246.b02"></a>
### S246.B02 — Gezielter Registryzugriff bleibt klein

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Entwurf bietet stabile IDs und begrenzte paginierte Abfrage, Aufgabe braucht nicht alle Namen gleichzeitig im Modell

**Warum bleibt oder endet der Zustand?** Ein konkreter Nachschlagevorgang endet begrenzt, weitere Seiten sind neue lesbare Anfragen

**Zugeordnete Residues:** [OBR027: Adressierbarer Namensbestand](residues.md#obr027)

**Was bleibt warum nutzbar?** Agent kann benötigten Namen aus großem Bestand finden ohne Gesamtmenge im Kontext, bei Vollmengenpflicht kein Ersatz

**Zu prüfen:** Gesuchter existierender Name ist durch versteckte Abschneidung nicht mehr abrufbar oder Nachricht wächst linear mit Gesamtregister

**Architekturfolge für diesen Stressor:** Registryabfrage und Kontexttext getrennte Verträge. Historischer compile-Pfad erhält explizite Quellen, eine automatische Vollregistrysendung ist dort nicht belegt.

<a id="s247"></a>
## S247 — Ein einziges Ergebnis enthält hundert Gigabytes in einem gültigen JSON-Feld

**Ursprung:** observability; A6-Zustände: OB27, OB25, OB35. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s247.b01"></a>
### S247.B01 — Gültiges Riesen-JSON erschöpft Parser

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Parser allokiert 100 GB vor Grenze, geteilter Speicher reicht nicht und letzte bestätigte Diskdaten bleiben intakt

**Warum bleibt oder endet der Zustand?** Verarbeitung stoppt bis Ressourcen frei oder Input verworfen, gültige Syntax liefert keinen Kapazitätsvertrag

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Alter bestätigter Bestand bleibt nach Entlastung lesbar, Riesenresultat und Controlservice sind nicht geschützt

**Zu prüfen:** Peakallokation bleibt vor Bodyannahme belegbar klein und Parser lehnt früh ab

<a id="s247.b02"></a>
### S247.B02 — Kleines Protokoll bleibt bedienbar, Body abgelehnt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Grenze greift vor Body- und Dekodierallokation, Gesamtpuffer sowie Controlpfad sind getrennt begrenzt

**Warum bleibt oder endet der Zustand?** Riesenresultat endet als Ablehnung, neue kleine zulässige Frames können unabhängig passieren

**Zugeordnete Residues:** [OBR028: Begrenzte Nachrichtenannahme](residues.md#obr028), [OBR019: Bedienbarer Kontrollpfad](residues.md#obr019)

**Was bleibt warum nutzbar?** Empfänger und Operator können kleine Antworten und Kontrollen nutzen, die 100-GB-Nutzlast selbst wird ausdrücklich nicht als erhalten gezählt

**Zu prüfen:** Parser oder referenzierter Blobabruf allokiert Bodygröße oder Riesenframe blockiert weiterhin Stop

**Architekturfolge für diesen Stressor:** Framegrenze vor Allokation und dekodierte Gesamtgröße prüfen. Inhaltsverweis ist keine automatische Erhaltung oder sichere unbegrenzte Downloadberechtigung.

<a id="s248"></a>
## S248 — Hunderttausend langsame Subscriber halten je eine eigene Queue offen

**Ursprung:** observability; A6-Zustände: OB27, OB11, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s248.b01"></a>
### S248.B01 — Viele einzeln kleine Queues erschöpfen Gesamtpool

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** 100000 langsame Leser halten eigene Queues, Summenspeicher unbeschränkt und bestätigter Store bleibt intakt

**Warum bleibt oder endet der Zustand?** Langsame laufende Nachfrage hält Belegung, Reconnectschleife wäre zusätzliche Annahme

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Bestandslesbarkeit kann nach Entlastung fortbestehen, laufende Benachrichtigungsabdeckung ist nicht erhalten

**Zu prüfen:** Gesamte Subscriberbytes bleiben bei steigender Leserzahl tatsächlich begrenzt

<a id="s248.b02"></a>
### S248.B02 — Begrenztes Fenster statt verlustfreier Einzelqueues

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Gemeinsames Ereignisfenster und Leseraufnahme haben globale Limits, Cursor ist bekannt und Lückenregel erlaubt Resync

**Warum bleibt oder endet der Zustand?** Langsame Leser werden abgewiesen oder sehen explizite Lücke statt unendlicher Akkumulation

**Zugeordnete Residues:** [OBR029: Begrenztes Cursor-Abonnement](residues.md#obr029)

**Was bleibt warum nutzbar?** Zugelassener Subscriber kann noch vorhandenen Suffix lesen, abgelaufene Benachrichtigungen sind nicht rekonstruiert

**Zu prüfen:** Cursor vor Fensteranfang erhält stillschweigend lückenlosen Erfolgsstatus oder Gesamtmemory wächst mit unbegrenzt neuen Lesern

**Architekturfolge für diesen Stressor:** Gesamtzahl Leser Gesamtbytes und Retentionsfenster zusammen begrenzen. Per-Queue-Cap allein ist kein globales Limit, alte Cursor brauchen ehrliche Lücke.

<a id="s249"></a>
## S249 — Zwanzig Jahre Ereignisse brauchen drei Wochen Replay bei vereinbartem RTO von einer Stunde

**Ursprung:** observability; A6-Zustände: OB23, OB09, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s249.b01"></a>
### S249.B01 — Korrektes Vollreplay endet nach verlorenem RTO

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Intakte lesbare kanonische Historie ist fest, Replay braucht drei Wochen ohne neue Events, Ziel war eine Stunde

**Warum bleibt oder endet der Zustand?** Vergangenes RTO bleibt verfehlt auch wenn endliches korrektes Replay später fertig wird

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Recovery kann erhaltene Geschichte für spätere Wiederherstellung nutzen, keine rechtzeitige Verfügbarkeit und kein nachgewiesenes heutiges Eventreplay

**Zu prüfen:** Vollständige Bereitschaft aus genau diesem Eingang ist einschließlich Prüfungen schon innerhalb einer Stunde erreicht

<a id="s249.b02"></a>
### S249.B02 — Vorhandener geprüfter Checkpoint verkürzt Rest

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorab existierender kompatibler Checkpoint plus vollständiger Suffix passen und gemessenes Replay inklusive Validierung bleibt unter RTO

**Warum bleibt oder endet der Zustand?** Endlicher Suffix wird ohne Pluginaufrufe angewandt, Bereitschaft erst nach bestandener Gleichwertigkeitsprüfung

**Zugeordnete Residues:** [OBR030: Prüfbarer Replay-Einstieg](residues.md#obr030)

**Was bleibt warum nutzbar?** Recovery kann verifizierten Einstieg nutzen statt Jahrzehnte erneut abspielen, fehlender Checkpoint darf nicht nachträglich angenommen werden

**Zu prüfen:** Checkpoint plus Suffix weicht vom Vollreplay ab oder Prüfung und Restreplay überschreiten eine Stunde

<a id="s249.b03"></a>
### S249.B03 — Beweglicher Replayrand holt nicht auf

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Neue Events kommen dauerhaft schneller als Restreplay verarbeitet, Historie bleibt lesbar

**Warum bleibt oder endet der Zustand?** Äußere Produktionsrate vergrößert Abstand, Ende der Neuzugänge ist eigene geänderte Bedingung

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Erhaltene Geschichte bleibt wiederaufbaubar bei endlichem Eingang, aktuelle Catch-up-Fähigkeit ist nicht erhalten

**Zu prüfen:** Bei gleichen Raten sinkt nachgewiesener Abstand dauerhaft auf null

**Architekturfolge für diesen Stressor:** RTO separat von Replaykorrektheit und Datenerhaltung nachweisen. Checkpoint nur mit erhaltenem Suffix Decoder und gemessener Bereitschaft als Entwurf akzeptieren.

<a id="s250"></a>
## S250 — Der Restore passt nur wenn Original Export temporäre Kopie und Index nie gleichzeitig existieren

**Ursprung:** observability; A6-Zustände: OB23, OB01, OB08. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s250.b01"></a>
### S250.B01 — Keine sichere gleichzeitige Kopienbelegung möglich

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Verwendeter Restore braucht mehr physisch gleichzeitigen Platz als vorhanden, einzige geprüfte Eingabe wird nicht gelöscht

**Warum bleibt oder endet der Zustand?** Halt dauert bis Kapazität oder tatsächlich sicherer anderer Stufenplan verfügbar ist

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann letzte verifizierte Quelle lesen, aktueller Restorefortschritt ist nicht möglich

**Zu prüfen:** Gemessener aktueller Algorithmus passt inklusive Scratch Index und Rollback ohne Quelle zu gefährden

<a id="s250.b02"></a>
### S250.B02 — Geprüfte Stufenfolge passt tatsächlich

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Expliziter Plan vermeidet unzulässige Überlappung, jede Ersatzstufe wird vor Löschen ihrer Vorgängerquelle geprüft und Peak passt

**Warum bleibt oder endet der Zustand?** Endliche Schritte erreichen Restore mit belegten Wiederanlaufpunkten, Zwischenfehler behalten mindestens eine gültige Quelle

**Zugeordnete Residues:** [OBR031: Raumverträglicher Wiederherstellungsplan](residues.md#obr031)

**Was bleibt warum nutzbar?** Operator kann Wiederherstellung mit vorhandenem Platz fortsetzen ohne imaginären Zusatzspeicher, Endgröße allein wäre kein Nachweis

**Zu prüfen:** Abbruch an einer Stufe lässt keine gültige Quelle oder tatsächliche COW-/Scratchbelegung übersteigt Kapazität

<a id="s250.b03"></a>
### S250.B03 — Letzte Quelle für Platz gelöscht und Ersatz unbrauchbar

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Unsicherer Plan löscht letzte gültige Datenquelle bevor brauchbarer Ersatz existiert, keine weitere lesbare Kopie im Inventar

**Warum bleibt oder endet der Zustand?** Benötigter Datenstand ist unter diesem geschlossenen Inventar nicht rekonstruierbar

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Wiederherstellung dieses verlorenen Standes bleibt keine Struktur, neue Backups oder perfekte Reparaturquelle dürfen nicht erfunden werden

**Zu prüfen:** Eine tatsächlich vorhandene rechtmäßig lesbare geprüfte Quelle liefert den verlorenen Stand

**Architekturfolge für diesen Stressor:** Peak-Live-Set mit Rollback und letzter guter Quelle planen. Kein Platzgewinn durch Löschen der einzigen verifizierten Eingabe als sichere Recovery verkaufen.

<a id="s251"></a>
## S251 — Um Platz zu sparen werden nur Operationstombstones gelöscht und der nächste Retry dupliziert Wirkung

**Ursprung:** observability; A6-Zustände: OB28, OB17, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s251.b01"></a>
### S251.B01 — Tombstones weg und Retry doppelt angenommen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Lokale Tombstones sind letzte lokale Zuordnung, Teilnehmer dedupliziert nicht mehr oder Retry benutzt neue Identität, zweite irreversible Wirkung tatsächlich angenommen

**Warum bleibt oder endet der Zustand?** Historische Nichtduplikation ist verloren, ein späterer Tombstone kann vergangene Annahme nicht löschen

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für ursprüngliche Einmalwirkung bleibt kein Residue, spätere Kompensation könnte nur Folgen mindern und ist nicht vorausgesetzt

**Zu prüfen:** Teilnehmerannahmebuch belegt dass kein zweiter Effekt akzeptiert wurde

<a id="s251.b02"></a>
### S251.B02 — Lokaler Verlust, Teilnehmer hält alte Identität

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Gegenbedingung zur vorgegebenen Duplikation: Teilnehmer hält gleiche stabile Retryidentität noch atomar und autorisiert abfragbar trotz lokaler Löschung

**Warum bleibt oder endet der Zustand?** Retry liefert alte Annahme statt neuer Wirkung bis Ende dieses Retentionshorizonts

**Zugeordnete Residues:** [OBR018: Teilnehmerseitiges Annahmebuch](residues.md#obr018)

**Was bleibt warum nutzbar?** Teilnehmer kann genau die doppelte Annahme verhindern und Recovery Ausgang zuordnen, lokales tombstonefreies System ist nicht allgemein sicher

**Zu prüfen:** Retryidentität ändert sich oder Teilnehmer hat den Eintrag vor Nachzügler bereits gelöscht

**Architekturfolge für diesen Stressor:** Tombstonebedarf nach spätestem möglichen Retry statt nach geringem Bytevolumen bestimmen. Teilnehmerhorizont getrennt belegen, kein Wiederherstellen gelöschter Zuordnung behaupten.

<a id="s252"></a>
## S252 — Nach zehn Jahren Pause sind eine Milliarde Schedule-Occurrences nachzuholen

**Ursprung:** observability; A6-Zustände: OB30, OB12, OB07, OB01, OB09. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s252.b01"></a>
### S252.B01 — Endlicher zwingend einzeln abzuarbeitender Altbestand

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Vertrag verlangt alle Milliarde Vorkommen einzeln, deren Absichten und Ursprungsfristen sind erhalten, tatsächliches Freigabegate hält abgelaufene Effektaufträge, Bearbeitungsrate freigegebener Arbeit positiv und kein unendlicher neuer Nachholhorizont

**Warum bleibt oder endet der Zustand?** Endlicher Rückstau kann langsam drainieren, historische Fristen sind dennoch verloren und Freigaben können Halt erfordern

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001), [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007), [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator kann erhaltene Vorkommen prüfen und ungültige Aktionen halten, keine schnelle Kapazität oder Milliarde Erfolge erhalten

**Zu prüfen:** Tatsächlicher Vertrag erlaubt übersprungene Intervalle oder Materialisierung erzeugt nach festem Ende ohne neue Quelle unbegrenzt weiter

<a id="s252.b02"></a>
### S252.B02 — Erlaubtes Intervall disponiert, nur kleiner Rest aufgenommen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ursprünglicher legitimer Vertrag erlaubt Skip oder Coalesce, kompakter Cursor und Dispositionsbeleg erhalten Intervall eindeutig, frische Erlaubnis vor Effekten

**Warum bleibt oder endet der Zustand?** Altes Intervall endet als deklarierte Disposition und nächste begrenzte Materialisierung beginnt ohne Doppelzählung

**Zugeordnete Residues:** [OBR032: Verdichteter Nachholstand](residues.md#obr032), [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007)

**Was bleibt warum nutzbar?** Scheduler kann Nachholstand und berechtigten kleinen Rest nutzen, übersprungene Vorkommen sind ausdrücklich nicht ausgeführte Erfolge

**Zu prüfen:** Neustart materialisiert dasselbe disponierte Intervall erneut oder Skip wird trotz Pflicht-Einzelausführung als vollständig ausgeführt gemeldet

**Architekturfolge für diesen Stressor:** Vergangenes Scheduleintervall von einer Milliarde ausführbaren Runs trennen. Skip/Coalesce braucht legitimen Vertrag und erteilt keine neue externe Freigabe.

<a id="s253"></a>
## S253 — Eine Providerstörung endet und alle blockierten Scopes starten im selben Millisekundenfenster

**Ursprung:** observability; A6-Zustände: OB12, OB11, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s253.b01"></a>
### S253.B01 — Endliche Entblockungskohorte drainiert

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Provider tatsächlich repariert, freigegebene Kohorte endlich und gemeinsame Aufnahme-/Versuchsbudgets erzwingen tragbare Last

**Warum bleibt oder endet der Zustand?** Zugelassene Aufträge enden oder werden wahr abgewiesen, ohne neue Retries verschwindet ursprünglicher Stoß

**Zugeordnete Residues:** [OBR016: Abgetrennte Restkapazität](residues.md#obr016), [OBR022: Gemeinsames Versuchsbudget](residues.md#obr022)

**Was bleibt warum nutzbar?** Scheduler kann begrenzt erlaubte Arbeit bedienen und Restversuche zählen, gleichzeitige sofortige Erfüllung aller Scopes nicht zugesagt

**Zu prüfen:** Nach vollständig disponierter Kohorte entstehen ohne neue Erlaubnis weitere Versuche

<a id="s253.b02"></a>
### S253.B02 — Gemeinsame Fehlerwellen erzeugen nächste Entblockung

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Deklarierter Grundbedarf wäre tragbar, synchrones Dispatch überschreitet Providerkapazität und Fehler setzen neue gemeinsame Retryepochen, Intent lesbar

**Warum bleibt oder endet der Zustand?** Stoß löst Quotenfehler aus, diese erneute gemeinsame Warte-/Freigabezeit, deren Stoß erhält Fehler nach ursprünglicher Reparatur

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann verbliebene Absichten prüfen, Durchsatz und Providerbudget sind nicht gesichert

**Zu prüfen:** Bei behobenem Initialfehler gleicher Grundlast und Policy drainiert jeder getestete Rückstau ohne erneute Fehlerwelle, Null-Ankünfte separat variieren

**Architekturfolge für diesen Stressor:** Provider-Rückkehr ist gemeinsamer Aufnahmeimpuls, keine neue Kapazität. Scopeberechtigung liefert keine Ressourcenisolation und Fehlerende muss bei gleichem Grundbedarf geprüft werden.

<a id="s254"></a>
## S254 — Ein viraler Auftrag erzeugt rekursiv Millionen Kinder bevor die Rechnung sichtbar wird

**Ursprung:** observability; A6-Zustände: OB36, OB11, OB30, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s254.b01"></a>
### S254.B01 — Großer aber endlicher legaler Baum

**Art:** transient. **Residue-Status:** teilweise.

**Voraussetzungen:** Endliche Scopezahl mit unverändert vererbter Nichtwiederholungskette, großer Verzweigungsfaktor und keine neuen Wurzeln, aufgenommene Absichten lesbar

**Warum bleibt oder endet der Zustand?** Jede Ebene hat weniger erlaubte Scopes, endliche Tiefe endet trotz Millionen Kindern, danach bleibt endlicher Rückstau

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann aufgenommenen Bestand prüfen, ursprüngliche Kosten- und Zeitgrenze kann längst verfehlt sein

**Zu prüfen:** Kinder dürfen dieselbe Scopekette zurücksetzen oder neue Wurzeln erzeugen, dann gilt Endlichkeitsannahme nicht

<a id="s254.b02"></a>
### S254.B02 — Neue Wurzeln reproduzieren Wachstum

**Art:** eskalation. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Kinder dürfen frische Wurzeln mit erneutem Kontingent erzeugen, mittlere neue Aufnahme über eins und kein Gesamtlimit

**Warum bleibt oder endet der Zustand?** Arbeit erzeugt neue erlaubte Wurzeln, diese mehr Kinder und Wurzeln, Wachstum bis realer Ressourcen- oder Autoritätsgrenze

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Ohne begrenzte durable Aufnahme ist weder vollständige Lineage noch nutzbare Restkapazität belegt, keine stabile Attraktorkonvergenz aus Wachstum behauptet

**Zu prüfen:** Jede neue Wurzel konsumiert dasselbe endliche autorisierte Gesamtkontingent oder effektive Reproduktion fällt unter eins

<a id="s254.b03"></a>
### S254.B03 — Begrenzter aufgenommener Baum bleibt prüfbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Jedes Kind erbt Wurzel und durchgesetztes Gesamt-Nachkommen-/Kostenbudget, frische Wurzeln brauchen eigene begrenzte Erlaubnis

**Warum bleibt oder endet der Zustand?** Budgetende stoppt weitere Aufnahme, bereits zugelassene Kinder bleiben endlicher zu disponierender Bestand

**Zugeordnete Residues:** [OBR033: Endliches Delegationskontingent](residues.md#obr033)

**Was bleibt warum nutzbar?** Dispatcher kann Restkontingent und Lineage nutzen bevor Rechnung kommt, verborgene externe Kosten bleiben außerhalb nachgewiesenem Modell

**Zu prüfen:** Breite Delegation oder erlaubter Frischwurzelpfad vervielfacht Budget ohne neue Gesamtfreigabe

**Architekturfolge für diesen Stressor:** Vererbte Scopekette begrenzt Tiefe aber nicht Breite. Gesamtkontingent an Wurzel binden und frische Wurzeln nur unter separat begrenzter Autorität zulassen.

<a id="s255"></a>
## S255 — Dauerlast drosselt die CPU auf ein Zehntel und verlängert alle Fristen gleichzeitig

**Ursprung:** observability; A6-Zustände: OB31, OB09, OB11, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s255.b01"></a>
### S255.B01 — Äußere Dauerlast hält niedrige Taktrate

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Last heizt CPU auf Zehntelrate, Kühlung erreicht nur dieses Gleichgewicht oder Zyklus, Zeitbelege erhalten

**Warum bleibt oder endet der Zustand?** Last erzeugt Wärme, Drossel senkt Leistung und Kühlung kann gegenwirken, fortgesetzte Last hält Regime ohne nötige Retryschleife

**Zugeordnete Residues:** [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Operator kann ursprüngliche Zeitverträge und gemessene Verfehlung weiter lesen, volle Warmkapazität ist nicht erhalten

**Zu prüfen:** Bei festem Bedarf korrelieren Temperatur Takt und Durchsatz nicht mit behaupteter Drosselwirkung

<a id="s255.b02"></a>
### S255.B02 — Zulassung passt zu gemessener gedrosselter Kapazität

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Physisches Aufnahmebudget wurde auf nachhaltige Zehntelkapazität ausgelegt, Mindestservice teilbarer Klassen ist wirklich erzwingbar

**Warum bleibt oder endet der Zustand?** Weniger zugelassene Last kann stabil bedient werden, abgewiesene Nachfrage ist nicht erfüllt

**Zugeordnete Residues:** [OBR016: Abgetrennte Restkapazität](residues.md#obr016), [OBR026: Garantierter begrenzter Dienstanteil](residues.md#obr026)

**Was bleibt warum nutzbar?** Zugelassene Klassen behalten begrenzten Service auch unter Drossel, frühere höhere Frist-/Mengenzusage gilt nicht automatisch

**Zu prüfen:** Gedrosselte CPU oder geteilter unteilbarer Engpass entzieht auch zugesagtem Rest jeden Service

<a id="s255.b03"></a>
### S255.B03 — Retries koppeln Hitze und lange Queues

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Deklarierter Grundbedarf wäre bei abgekühltem System tragbar, Timeouts erneuern zusätzliche Arbeit und verhindern Abkühlung, Intent lesbar

**Warum bleibt oder endet der Zustand?** Hitze senkt Rate, Queue erzeugt Timeouts, Retryarbeit erhält Auslastung und Hitze und damit neue Timeouts

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001), [OBR012: Unverjüngter Zeitvertrag](residues.md#obr012)

**Was bleibt warum nutzbar?** Bestands- und Zeitdiagnose bleiben möglich, weder Deadlineerhalt noch selbstständige Abkühlung sind gesichert

**Zu prüfen:** Nach Initialstörung bei gleicher Grundlast und Policy kühlt jeder getestete Rückstau ab und drainiert, Null-Neuzugänge erst separat prüfen

**Architekturfolge für diesen Stressor:** Thermisch nachhaltige Rate statt kurzer Warmbenchmark als Zulassungsbasis messen. CPU-Drossel ändert reale Dauer, nicht automatisch Vertragsablauf.

<a id="s256"></a>
## S256 — Die USV reicht für den Commit aber nicht für den nachfolgenden externen Versand

**Ursprung:** observability; A6-Zustände: OB16, OB06, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s256.b01"></a>
### S256.B01 — Dauerhafter Versuch, Versandpunkt unbekannt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** USV reicht für lokales Intentcommit, Recovery kann nicht unterscheiden ob Writer gar nicht aufgerufen oder externe Annahmeantwort verloren ging

**Warum bleibt oder endet der Zustand?** Unbekannter Ausgang hält automatische Wiederholung bis korrelierte neue Evidenz, Stromrückkehr allein beendet Wissenlücke nicht

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Recovery kann belegten Versuch zurückhalten und lesen, weder Mailzustellung noch No-send sind dadurch erhalten

**Zu prüfen:** Erhaltener unabhängiger Beleg bestimmt eindeutig ob Annahmepunkt erreicht wurde

<a id="s256.b02"></a>
### S256.B02 — Teilnehmerbeleg entscheidet tatsächlichen Ausgang

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Teilnehmer führt für dieselbe Identität erhaltenes autorisiert abfragbares Annahmebuch und definitive Nichtannahme braucht geschlossene weitere Annahmemöglichkeit

**Warum bleibt oder endet der Zustand?** Passende Evidenz beendet genau Ausgangsfrage, zulässige erneute Aktion braucht gegebenenfalls neue Freigabe

**Zugeordnete Residues:** [OBR018: Teilnehmerseitiges Annahmebuch](residues.md#obr018)

**Was bleibt warum nutzbar?** Operator kann Annahme oder belegte endgültige Nichtannahme unterscheiden, bloß momentanes not-found bei weiterlaufendem Sender reicht nicht

**Zu prüfen:** Not-found wird als No-send ausgegeben obwohl alter Teilnehmer später noch annehmen darf

**Architekturfolge für diesen Stressor:** Commit-vor-Writer-Lücke funktionsgebunden halten: historische deliver-Quelle nutzt manuelle Übergabe, keinen bewiesenen Versand. UPS kann weder Fernannahme noch deren Nichtvorkommen aus lokalem Intent beweisen.

<a id="s257"></a>
## S257 — Lastabwurf schaltet zuerst den als unwichtig eingestuften Monitoringrouter ab

**Ursprung:** observability; A6-Zustände: OB02, OB03, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s257.b01"></a>
### S257.B01 — Router aus, Geschäft läuft ohne Außenblick

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur Monitoringrouter wird abgeworfen, Geschäftsweg und bestätigter Bestand bleiben nutzbar, kein anderer Beobachtungspfad

**Warum bleibt oder endet der Zustand?** Beobachtungsblindheit bleibt mit Abwurf, Geschäftsfortschritt kann unabhängig weitergehen

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Lokaler Operator kann bestätigten Bestand nutzen und Arbeit kann laufen, daraus folgt kein frischer Remotezustand oder Menschenempfang

**Zu prüfen:** Abwurf stoppt tatsächlich auch Geschäftsweg oder frischer unabhängiger Außenbefund bleibt vorhanden

<a id="s257.b02"></a>
### S257.B02 — Anderer konkreter Beobachtungspfad überlebt Abwurf

**Art:** extern-erzwungen. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorab vorhandener Zeuge samt eigener Verbindung Energie Identität und Lesezugang hängt nicht am abgeworfenen Router

**Warum bleibt oder endet der Zustand?** Zeuge erhält begrenzte frische Befunde oder Ausfallevidenz auch bei anhaltendem Abwurf

**Zugeordnete Residues:** [OBR002: Unabhängige Ausfallfeststellung](residues.md#obr002)

**Was bleibt warum nutzbar?** Externer Leser kann definierte Erreichbarkeit beobachten, Produktionsausführung und Empfang werden separat ausgewiesen

**Zu prüfen:** Alternativer Pfad routet doch durch abgeschaltetes Gerät oder teilt dessen Versorgung

**Architekturfolge für diesen Stressor:** Lastabwurf muss Monitoringenergie/-netz als Abhängigkeit explizit machen. OB02 wird nur auf Beobachtung bezogen, Ausführung kann vollständig weiterlaufen.

<a id="s258"></a>
## S258 — Stop wartet hinter sämtlichen überlasteten Pluginaufrufen

**Ursprung:** observability; A6-Zustände: OB35, OB11, OB01. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s258.b01"></a>
### S258.B01 — Stop hinter dauernd belegten Aufrufen

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** Stop teilt voll belegten Pool, Calls hängen oder werden durch dauernden Input ersetzt, Journal bereits bestätigter Versuche lesbar

**Warum bleibt oder endet der Zustand?** Ohne freien Service wartet Stop, endliche Calltimeouts könnten transient freigeben, erneute Retrylast wäre zusätzliche Schleife

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Operator kann frühere Versuche später abgleichen, aktuelle Notsteuerbarkeit ist nicht erhalten

**Zu prüfen:** Stop wird unter derselben unveränderten Poolbelegung innerhalb seiner zugesagten Frist tatsächlich bedient

<a id="s258.b02"></a>
### S258.B02 — Reservierte Kontrolle sperrt neue Aufnahme

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Controlpfad Timer und Supervisor sind unabhängig bedienbar, Dispatchsperre greift bevor weitere Calls zugelassen werden

**Warum bleibt oder endet der Zustand?** Neue Calls bleiben gehalten während bestehende getrennt beendet oder reconciliert werden, Stopstatus bleibt bis Wirkung unvollständig

**Zugeordnete Residues:** [OBR019: Bedienbarer Kontrollpfad](residues.md#obr019)

**Was bleibt warum nutzbar?** Operator kann Belastungsnachschub stoppen und Abbruch anstoßen, abgeschlossener irreversibler Effekt wird nicht zurückgenommen

**Zu prüfen:** Gesättigter Pluginloop verhindert Timerlauf oder Stopannahme wird ohne Fence und Commit als vollständig ausgegeben

**Architekturfolge für diesen Stressor:** Stopaufnahme und Timer aus gesättigten Pluginpools heraus bedienbar machen, danach tatsächliche Autoritätswirkung und dauerhafte Bestätigung getrennt prüfen.

<a id="s259"></a>
## S259 — Der Notfallzugang verlangt einen Login beim gerade gesperrten einzigen Identitätsanbieter

**Ursprung:** observability; A6-Zustände: OB20, OB33, OB21. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s259.b01"></a>
### S259.B01 — Einziger Identitätsweg gesperrt

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** IdP blockiert und kein gültiger vorheriger lokaler Login oder Offlinecredential vorhanden

**Warum bleibt oder endet der Zustand?** Zugriff bleibt fail-closed bis berechtigte Wiederherstellung des Identitätswegs, keine autonome Schleife

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für aktuelle autorisierte Notbedienung ist kein nutzbarer Zugang vorhanden, Daten können dennoch intakt sein

**Zu prüfen:** Vorhandener gültiger lokaler Notnachweis erlaubt definierte Befehle ohne IdP

<a id="s259.b02"></a>
### S259.B02 — Vorab gebundene Offline-Notbefehle möglich

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Erreichbarer Operator besitzt bereits gültiges enges Offlinecredential und lokaler Prüfpfad arbeitet ohne gesperrten Provider

**Warum bleibt oder endet der Zustand?** Definierte Recoverykontrollen bleiben bis Ablauf oder Widerrufsgrenze nutzbar, übriger IdP-Zugang bleibt gesperrt

**Zugeordnete Residues:** [OBR034: Gebundener Offline-Notzugang](residues.md#obr034)

**Was bleibt warum nutzbar?** Operator kann autorisierte Notaktionen ausführen, keine anonyme Rettung und keine vollständige externe Accountwiederherstellung

**Zu prüfen:** Offlinepfad verlangt doch IdP oder ein unberechtigter Principal kann dieselben Befehle ausführen

**Architekturfolge für diesen Stressor:** Lokales IPC-Safe-mode ist kein Autorisierungsbypass. Offline-Notautorität muss vorher eng bereitstehen, sonst ehrlicher Halt statt offener Shell.

<a id="s260"></a>
## S260 — Die SSD ist voll und Factory bestätigt einen Stopp den es nicht speichern konnte

**Ursprung:** observability; A6-Zustände: OB29, OB08, OB16. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s260.b01"></a>
### S260.B01 — Commitfehler wird ehrlich zurückgegeben

**Art:** halt. **Residue-Status:** teilweise.

**Voraussetzungen:** SSD voll, direkte historische stop/cancel-Transaktion scheitert und Caller propagiert Fehler, alter Bestand lesbar

**Warum bleibt oder endet der Zustand?** Keine neue dauerhafte Stopzusage bis Speicher wieder schreibt, physischer Prozesszustand ist separate Frage

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Operator kann alten Stand lesen und fehlgeschlagene Mutation erkennen, kein dauerhafter Stop ist erhalten

**Zu prüfen:** Gesamter Clientpfad gibt trotz fehlgeschlagenem Commit erfolgreiche dauerhafte Stopbestätigung aus

<a id="s260.b02"></a>
### S260.B02 — Wrapper behauptet gespeicherten Stop ohne Commit

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Hypothetischer Wrapper bestätigt zu früh, SSD nimmt Stop nicht an, Prozess könnte tatsächlich gestoppt sein oder weiterlaufen

**Warum bleibt oder endet der Zustand?** Nach Restart kann alter gewünschter Zustand wieder wirksam werden, falsche Quittung entscheidet reale Wirkung nicht

**Zugeordnete Residues:** [OBR001: Lesbarer bestätigter Bestand](residues.md#obr001)

**Was bleibt warum nutzbar?** Nur frühere dauerhafte Daten sind prüfbar, versprochene persistente Stopgarantie ist verloren beziehungsweise nicht hergestellt

**Zu prüfen:** Erfolgsantwort hängt belegbar am erfolgreichen Stopcommit oder unabhängige dauerhaft gespeicherte Stopautorität existiert

<a id="s260.b03"></a>
### S260.B03 — Volatile Notkontrolle wirkt ohne Dauerhaftigkeitsbehauptung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Separater Supervisor kann laufenden Host kontrollieren und Dispatch vorübergehend sperren, aber SSD bleibt voll und Neustartschutz ist nicht gespeichert

**Warum bleibt oder endet der Zustand?** Physische Sperre hält nur solange diese Autorität lebt, Wiederanlauf muss erneut geprüft werden

**Zugeordnete Residues:** [OBR019: Bedienbarer Kontrollpfad](residues.md#obr019)

**Was bleibt warum nutzbar?** Operator kann gegenwärtigen Nachschub stoppen obwohl Persistenz fehlt, Antwort muss ausdrücklich nicht-dauerhaft bleiben

**Zu prüfen:** Neustart wird ohne neue Prüfung als weiterhin dauerhaft gestoppt bestätigt oder Control teilt den blockierten Schreibpfad

**Architekturfolge für diesen Stressor:** Dauerhaften gewünschten Stop von physischem Prozessende und Antwortstufe trennen. Historische stop/cancel-Library propagiert Commitfehler; lügender Wrapper ist nur Gegenbedingung.

<a id="s261"></a>
## S261 — Ein korrekt authentifizierter Pluginaufruf verwendet das Credential eines anderen Scopes

**Ursprung:** observability; A6-Zustände: OB33, OB17, OB21, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s261.b01"></a>
### S261.B01 — Fremdes Credential hat fremde Aktion ermöglicht

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Plugin A erhält oder nutzt B-Credential, externe unberechtigte Aktion wird akzeptiert und unabhängiges lokales Versuchsjournal bleibt intakt

**Warum bleibt oder endet der Zustand?** Vergangene falsche Autorisierung bleibt auch nach Credentialwiderruf, Offenlegung braucht zusätzlich tatsächlichen Klartextleser

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Operator kann belegte Versuche für Abgleich nutzen, korrekte Scopeautorität dieser Aktion ist nicht erhalten

**Zu prüfen:** Teilnehmer lehnt B-Nutzung vor Annahme ab oder Aktion war gesondert korrekt für B autorisiert

<a id="s261.b02"></a>
### S261.B02 — Credential-Gate bewahrt erlaubte A-Nutzung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Trusted Aktivierung bindet A Scope Zweck und Ziel, Anfrage auf B wird vor Release oder Nutzung abgewiesen, kein OS-Vaultbypass

**Warum bleibt oder endet der Zustand?** Falsche Anfrage endet, weiterhin erlaubte A-Nutzung kann separat passieren

**Zugeordnete Residues:** [OBR036: Scope-gebundene Credential-Nutzung](residues.md#obr036)

**Was bleibt warum nutzbar?** Secret-Gate kann passende A-Capability weiter bedienen ohne B-Zugang zu geben, Pipeauth allein wäre kein Nachweis

**Zu prüfen:** Authentischer A-Prozess liest B-Secret direkt oder wählt B erfolgreich über freies Requestfeld

**Architekturfolge für diesen Stressor:** Authentisierte Pipe an konkrete Secret-Capability von Scope und Zweck binden, nicht Pluginbehauptung. Falsche Nutzung und tatsächliche Offenlegung getrennte Wirkungen.

<a id="s262"></a>
## S262 — Ein menschenlesbarer Empfängername wird kurz vor Dispatch auf eine andere Identität umgebogen

**Ursprung:** observability; A6-Zustände: OB33, OB17, OB21, OB25, OB07. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s262.b01"></a>
### S262.B01 — Umgebogener Name erreicht falschen Empfänger

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Dispatch löst nur veränderbaren Anzeigenamen neu auf, andere reale Identität nimmt irreversible Aktion an, Versuchsjournal erhalten

**Warum bleibt oder endet der Zustand?** Vergangene falsche Empfängerwirkung bleibt, geheimhaltungsbezogener Zusatzverlust nur bei tatsächlicher sensibler Offenlegung

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Operator kann Versuch und bekannte Zielbelege prüfen, ursprüngliche Empfängertreue ist verloren

**Zu prüfen:** Tatsächlicher Teilnehmer bindet trotz Namensänderung unverändert den genehmigten Empfänger

<a id="s262.b02"></a>
### S262.B02 — Gebundene Zielidentität verhindert Umlenkung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Approval umfasst echte unveränderliche Zielidentität Version und Aktion, Gate prüft letzten kontrollierten Dispatch und Teilnehmer respektiert Identität

**Warum bleibt oder endet der Zustand?** Abweichung hält Aktion bis passende neue Freigabe, kosmetischer Name kann getrennt wechseln

**Zugeordnete Residues:** [OBR035: Aktionsgebundene Freigabeevidenz](residues.md#obr035), [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007)

**Was bleibt warum nutzbar?** Operator kann ursprüngliche Absicht und Freigabe nutzen ohne still auf neuen Empfänger zu senden

**Zu prüfen:** Teilnehmer löst gebundene ID hinter dem Gate in andere Person auf oder Namewechsel ändert Ziel ohne neue Freigabe

**Architekturfolge für diesen Stressor:** Menschlichen Namen vor Freigabe an unveränderliche tatsächliche Zielidentität binden und am Dispatch prüfen. Erneute Freigabe bei semantischer Änderung statt Namegleichheit.

<a id="s263"></a>
## S263 — Eine importierte Task-ID wird als Beleg einer fremden Freigabe akzeptiert

**Ursprung:** observability; A6-Zustände: OB33, OB17, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s263.b01"></a>
### S263.B01 — Fremde ID wird fälschlich Autorität

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Importer setzt bekannte fremde ID ein, Gate behandelt ID als Zustimmung und irreversible unberechtigte Aktion wird angenommen, unabhängiges Journal erhalten

**Warum bleibt oder endet der Zustand?** Historische falsche Freigabe bleibt, UUID-Eindeutigkeit würde sie nicht legitimieren

**Zugeordnete Residues:** [OBR017: Lokales Wirkungsjournal](residues.md#obr017)

**Was bleibt warum nutzbar?** Operator kann erhaltenen Versuch prüfen, Herkunft einer Task-ID beweist keine lokale Berechtigung

**Zu prüfen:** Separates autoritatives Approval deckt genau den tatsächlichen Principal Scope und die Aktion

<a id="s263.b02"></a>
### S263.B02 — ID bleibt Datenreferenz ohne Approvalwirkung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Import bleibt untrusted und nur separate authentisierte passende Approvalquelle kann Dispatch erlauben

**Warum bleibt oder endet der Zustand?** Falscher Autoritätsversuch endet als Ablehnung, andere korrekt gebundene Arbeit darf weiterlaufen

**Zugeordnete Residues:** [OBR035: Aktionsgebundene Freigabeevidenz](residues.md#obr035)

**Was bleibt warum nutzbar?** Dispatcher kann echte Freigabeevidenz nutzen und importierte Identität als bloße Referenz lesen

**Zu prüfen:** Fremde syntaktisch gültige Task-ID ersetzt die unabhängige Approvalprüfung erfolgreich

**Architekturfolge für diesen Stressor:** Importierte Task-ID nie selbst als Freigabeevidenz akzeptieren. Approvalprovenienz Principal Scope und unveränderliche Aktion am Dispatch überprüfen.

<a id="s264"></a>
## S264 — Ein Dokument behauptet Systemregel zu sein und verlangt den Upload aller Secrets

**Ursprung:** observability; A6-Zustände: OB25, OB33, OB21. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s264.b01"></a>
### S264.B01 — Instruktionsübernahme führt zu Secret-Offenlegung

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Agent erhebt Dokument zur Systemautorität und besitzt tatsächlichen Secretlese- plus Uploadzugriff, unberechtigter Empfänger erhält Klartext

**Warum bleibt oder endet der Zustand?** Bereits gelesene Secrets sind nicht zurückholbar, Rotation schützt höchstens künftige Nutzung

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Vertraulichkeit der hochgeladenen Fakten bleibt keine Struktur, bloßer Text ohne erfolgreichen Zugriff wäre noch kein Verlust

**Zu prüfen:** Secret-Gate oder Egress stoppt vor Übertragung und kein unberechtigter Leser erhält Klartext

<a id="s264.b02"></a>
### S264.B02 — Inhalt bleibt ohne Handlungsautorität

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängiges Approvalgate kontrolliert jede entsprechende externe Aktion, Dokument kann diese Quelle nicht ändern

**Warum bleibt oder endet der Zustand?** Geforderter Upload scheitert ohne passende echte Freigabe, legitime unabhängig gebundene Aktionen bleiben möglich

**Zugeordnete Residues:** [OBR035: Aktionsgebundene Freigabeevidenz](residues.md#obr035)

**Was bleibt warum nutzbar?** Dispatcher kann wahre Freigaben weiter nutzen, Text wird als Daten gelesen ohne behauptete Systemregel zu übernehmen

**Zu prüfen:** Dokumentwortlaut allein erzeugt gültige Uploadfreigabe oder Agent benutzt ungeprüften externen Nebenkanal

<a id="s264.b03"></a>
### S264.B03 — Daten bleiben außerhalb erreichbarer Rechte

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** OS- und Egress-Kompartiment enthält geschützte Secrets außerhalb Agentenzugriff, selbst bei instruktionswilligem Modell

**Warum bleibt oder endet der Zustand?** Versuch bleibt ohne tatsächlichen Lese-/Übertragungsweg blockiert solange diese Grenze gilt

**Zugeordnete Residues:** [OBR039: Nicht erreichbarer Datenbereich](residues.md#obr039)

**Was bleibt warum nutzbar?** Berechtigte Prozesse können nicht offengelegte Daten weiter nutzen, Modellgehorsam ist keine vorausgesetzte Schutzstruktur

**Zu prüfen:** Agent liest Secrets über gemeinsame UID Prozessinspektion oder zulässigen Nebenkanal und kann sie abführen

**Architekturfolge für diesen Stressor:** Dokumenttext darf keine Capability oder Approvalquelle sein. Promptformatierung nicht als Secret- oder Egressgrenze behandeln.

<a id="s265"></a>
## S265 — Ein Fehlerlog enthält gefälschte Bestätigungen die ein Recovery-Agent für Operatorfreigaben hält

**Ursprung:** observability; A6-Zustände: OB33, OB17, OB21, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s265.b01"></a>
### S265.B01 — Gefälschte Bestätigung ermöglicht Fehlaktion

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Recovery-Agent glaubt Logtext und besitzt effektiven Dispatchzugang, unberechtigte irreversible Aktion wird angenommen

**Warum bleibt oder endet der Zustand?** Vergangene Fehlaktion bleibt, plausible Logform beweist nie den behaupteten Operator

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für korrekte Autorisierung dieser angenommenen Aktion bleibt kein Residue, manipuliertes Log ist gerade keine unabhängige Beweisquelle

**Zu prüfen:** Dispatch verlangt und erhält eine echte separat verifizierte passende Zustimmung oder verweigert vor Annahme

<a id="s265.b02"></a>
### S265.B02 — Gefälschtes Log bleibt nicht autorisierende Diagnose

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Approvalbuch und Prüfgate liegen außerhalb veränderbarer Logtexte, Principal Scope Aktion und Gültigkeit werden unabhängig geprüft

**Warum bleibt oder endet der Zustand?** Log kann Diagnose verwirren aber keine neue Erlaubnis erzeugen, zulässige Recoveryentscheidung braucht echte Quelle

**Zugeordnete Residues:** [OBR035: Aktionsgebundene Freigabeevidenz](residues.md#obr035)

**Was bleibt warum nutzbar?** Recovery kann autoritative Freigabeevidenz weiter verwenden trotz falscher Diagnosetexte, Logwahrheit ist nicht gerettet

**Zu prüfen:** Passend formatierte Logzeile erfüllt ohne echte Operatorhandlung die Approvalabfrage

**Architekturfolge für diesen Stressor:** Logs sind Diagnose, keine Operatorstimmen. Wiederanlauffreigaben müssen aus separater authentisierter Quelle stammen und genau die neue Aktion binden.

<a id="s266"></a>
## S266 — Ein Ergebnisartefakt schmuggelt eine neue externe Aktion in seine Prüfanweisung

**Ursprung:** observability; A6-Zustände: OB33, OB17, OB21, OB25, OB07. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s266.b01"></a>
### S266.B01 — Prüfanweisung wird unerlaubte neue Aktion

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Prüfer folgt Artefaktinstruction über externes Tool ohne passendes Approval und irreversible Aktion wird angenommen

**Warum bleibt oder endet der Zustand?** Neue historische Aktion bleibt trotz späterer Ablehnung des Artefakts, Completionprüfung hatte sie nicht autorisiert

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für ursprüngliche Aktionsbegrenzung dieser angenommenen Aktion bleibt nichts, erhaltenes Artefakt allein ist keine vertrauenswürdige Freigabe

**Zu prüfen:** Tool-Gate verhindert Annahme oder eine unabhängige bereits gültige Zustimmung deckt genau diese Aktion

<a id="s266.b02"></a>
### S266.B02 — Neue Aktion als nicht freigegebene Absicht gehalten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Prüfer oder erzwungenes Gate behandelt Artefaktinhalt als untrusted Daten, neue Aktion braucht unabhängige gebundene Freigabe

**Warum bleibt oder endet der Zustand?** Prüfung kann getrennt fortgehen, zusätzliche Aktion bleibt bis echter Entscheidung gehalten oder wird verworfen

**Zugeordnete Residues:** [OBR035: Aktionsgebundene Freigabeevidenz](residues.md#obr035), [OBR007: Gehaltene Absicht mit fehlender Erlaubnis](residues.md#obr007)

**Was bleibt warum nutzbar?** Operator kann verlangte neue Absicht prüfen ohne sie aus Artefaktbytes zu autorisieren, bloße Formatgrenze wäre unzureichend

**Zu prüfen:** Artefaktinhalt kann ohne neue echte Zustimmung externe Wirkung auslösen

**Architekturfolge für diesen Stressor:** Artefaktprüfung und neue externe Aktion trennen. Hash oder gültiger Inhaltsverweis bindet Bytes, nicht Erlaubnis zu darin geforderten Tools.

<a id="s267"></a>
## S267 — Ein gültig signiertes Pluginupdate exfiltriert Daten

**Ursprung:** observability; A6-Zustände: OB21, OB22, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s267.b01"></a>
### S267.B01 — Signierter Einmalpayload legt Klartext offen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Gültig signierter Code hat Daten- und Egresszugriff und Empfänger erhält geschützte Daten, keine Persistenzschleife belegt

**Warum bleibt oder endet der Zustand?** Offenlegung ist historisch irreversibel auch wenn Payload danach endet

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Geheimhaltung tatsächlich exfiltrierter Daten bleibt keine Restfähigkeit, Einmalverlust ist kein Attraktor

**Zu prüfen:** Kein unberechtigter Empfänger erhält verwertbaren Klartext

<a id="s267.b02"></a>
### S267.B02 — Update und Recoveryevidenz reproduzieren Kompromiss

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Angreifer kontrolliert wiederkehrenden Update-/Persistenzpfad und für Recoveryentscheidungen verwendete In-Domain-Evidenz, keine unabhängige nutzbare Prüfbasis vorhanden

**Warum bleibt oder endet der Zustand?** Kompromittierter Updater installiert schädlichen Code, falsche Evidenz legitimiert Recovery, diese lädt denselben Kompromiss erneut

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für unabhängig glaubwürdige In-Domain-Recoveryentscheidung ist kein überlebender Gegenstand benannt, nicht automatisch jeder Datenbyte zerstört

**Zu prüfen:** Sauberer Neustart bleibt trotz fortbestehend behaupteter Reinstallationsautorität sauber oder Recovery benutzt wirksame unabhängige Evidenz

<a id="s267.b03"></a>
### S267.B03 — Signierter Code erreicht geschützten Bereich nicht

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Gegenbedingung zum tatsächlichen Exfiltrationserfolg: durchgesetzte OS-/Egressgrenze sperrt geschützte Daten und fremde Ausgänge, gültige eigene Funktion bleibt möglich

**Warum bleibt oder endet der Zustand?** Unerlaubter Zugriff scheitert vor Offenlegung solange Isolation außerhalb Pluginmacht liegt

**Zugeordnete Residues:** [OBR039: Nicht erreichbarer Datenbereich](residues.md#obr039)

**Was bleibt warum nutzbar?** Berechtigte Prozesse können geschützten nicht offengelegten Bereich weiter nutzen, Signatur allein leistet das nicht

**Zu prüfen:** Signed Payload umgeht Grenze über gleiche UID privilegierte Rechte oder erlaubten Klartextkanal

**Architekturfolge für diesen Stressor:** Signatur prüft Herkunft, nicht Verhalten. Schon offengelegte Daten nicht als durch nachträgliche Isolation erhalten verkaufen, Wiederinfektion braucht konkrete reproduzierende Rechte.

<a id="s268"></a>
## S268 — Ein zurückgezogenes Paket wird zwanzig Jahre später für Restore aus einem übernommenen Namen geladen

**Ursprung:** observability; A6-Zustände: OB21, OB22, OB25, OB23. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s268.b01"></a>
### S268.B01 — Übernommener Paketname startet fremden Code

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Restore vertraut nur wiederverwendetem Namen beziehungsweise aktueller Signatur, neuer Code liest und exfiltriert tatsächlich Daten

**Warum bleibt oder endet der Zustand?** Historische Offenlegung bleibt, Namensübernahme allein ohne Ausführung und Zugriff wäre nur Exposition

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Geheimhaltung dieser gelesenen Daten ist verloren, eine alte unverfügbare Kopie wird nicht als Rettung erfunden

**Zu prüfen:** Restore lehnt fremde Bytes vor Ausführung ab oder Code erhält keinen geschützten Klartext

<a id="s268.b02"></a>
### S268.B02 — Erhaltene Inhaltsbytes lassen kontrollierten Restore zu

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Verifizierbare alte ausführbare Bytes Prüfanker und kompatible Runtime wurden vorab unabhängig erhalten, bekannte Bytes sind für diesen Zweck vertrauenswürdig geprüft

**Warum bleibt oder endet der Zustand?** Restore benutzt festen Inhalt statt neu besetztem Namen, Abschluss gilt nur bei bestandener Laufzeitprüfung

**Zugeordnete Residues:** [OBR037: Verifizierbare ausführbare Archivbytes](residues.md#obr037)

**Was bleibt warum nutzbar?** Recovery kann konkret vorhandene Abhängigkeit wieder benutzen, Inhaltsgleichheit allein beweist keine Benignität

**Zu prüfen:** Digest stimmt nicht Laufzeit fehlt oder ursprünglicher Inhalt war selbst bösartig

<a id="s268.b03"></a>
### S268.B03 — Prüfanker erhalten, benötigte alte Bytes fehlen

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Nur vertrauenswürdiger Digest ist erhalten, keine bekannte zugängliche Kopie ausführbarer alter Bytes, neuer Name liefert andere Bytes

**Warum bleibt oder endet der Zustand?** Korrekte Ablehnung hält Restore bis tatsächliche passende Bytes gefunden werden, Gegenwartsmangel beweist noch kein ewiges Allweltverschwinden

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Ausführung der benötigten alten Abhängigkeit fehlt der nutzbare Gegenstand, bloßer Hash ist kein ausführbares Residue

**Zu prüfen:** Eine tatsächlich vorhandene verifizierbare und kompatibel ausführbare Kopie wird gefunden

**Architekturfolge für diesen Stressor:** Archivierte Inhalte Prüfanker und ausführungstaugliche Laufzeit getrennt sichern. Digestablehnung kann korrekt sein während vollständiger Restore mangels Bytes unmöglich bleibt.

<a id="s269"></a>
## S269 — Das unabhängige Monitoring erhält denselben kompromittierten Auto-Updater wie Factory

**Ursprung:** observability; A6-Zustände: OB03, OB22, OB21. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s269.b01"></a>
### S269.B01 — Gemeinsames Update korrumpiert beide Befunde

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Schädlicher Updater hat Factory und Monitor erreicht, Monitor kann falsches Grün erzeugen, weitere Reinstallation nicht belegt

**Warum bleibt oder endet der Zustand?** Aktuelles In-Domain-Grün ist unzuverlässig bis andere glaubwürdige Evidenz oder saubere Basis entsteht, Einmalinstallation ist noch keine Schleife

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für unabhängige Gesundheitsbeurteilung durch genau diesen Monitor bleibt keine glaubwürdige Struktur, Ausführung und totale Datenzerstörung folgen daraus nicht

**Zu prüfen:** Monitoradministration war tatsächlich außerhalb kompromittierter Updaterrechte und kann relevante Abweichung unabhängig belegen

<a id="s269.b02"></a>
### S269.B02 — Reinstallation bestätigt sich selbst

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Updater behält Persistenzrechte über beide Systeme und Operator-Recoveryentscheidungen verlassen sich wiederholt auf deren manipulierte Befunde

**Warum bleibt oder endet der Zustand?** Schädliches Update erzeugt falsches Gesundsignal, dieses erlaubt in derselben Domäne Recovery, Update installiert erneut schädlichen Stand

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für unabhängige Vertrauensentscheidung aus dieser geschlossenen Domäne ist nichts belegt, keine automatische globale Informationsvernichtung

**Zu prüfen:** Bei fortbestehender behaupteter Macht bleibt unabhängig geprüfter Neustart sauber oder falsches Grün beeinflusst keine Recoveryentscheidung

<a id="s269.b03"></a>
### S269.B03 — Zusätzlicher anders verwalteter Zeuge erkennt definierte Abweichung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorab vorhandene Prüfbasis samt Administration liegt außerhalb gemeinsamen Updaters und konkrete schädliche Änderung ist durch ihre Beobachtungen unterscheidbar

**Warum bleibt oder endet der Zustand?** Zeuge bleibt glaubwürdig solange Angreifer weder Basis noch dessen Evidenzpfad ändern kann, Reparatur ist separater Schritt

**Zugeordnete Residues:** [OBR038: Anders verwalteter Nur-Lese-Zeuge](residues.md#obr038)

**Was bleibt warum nutzbar?** Berechtigter Operator kann beschränkten Gegenbefund nutzen trotz in-domain Grün, keine Erkennung beliebig unsichtbarer Exfiltration

**Zu prüfen:** Angreifer kontrolliert doch Zeugenupdate oder saubere und schädliche Ausführung liefern identische erlaubte Beobachtungen

**Architekturfolge für diesen Stressor:** Monitoring braucht getrennte administrative Vertrauensdomäne, nicht nur Strom/Netz. Unabhängige Evidenz muss eine konkrete Abweichung unterscheiden können, kein perfektes Orakel annehmen.

<a id="s270"></a>
## S270 — Heartbeat-Schlüsselrotation und Netzpartition überlappen sodass beide Seiten den jeweils anderen Key ablehnen

**Ursprung:** observability; A6-Zustände: OB20, OB02, OB01, OB03. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s270.b01"></a>
### S270.B01 — Partition geheilt, Schlüsselzustände bleiben unvereinbar

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Beide Seiten akzeptieren ausschließlich ihren unterschiedlichen Key und keine gültige Überlappung oder separate Rekeyautorität ist vorhanden

**Warum bleibt oder endet der Zustand?** Netzrückkehr ändert akzeptierte Keys nicht, fail-closed hält bis echte autorisierte Synchronisierung

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für gegenseitig authentisierte aktuelle Heartbeats besteht kein nutzbarer Zugang, Hostausführung kann trotzdem laufen

**Zu prüfen:** Unveränderte akzeptierte Keymengen validieren nach Netzrückkehr legitime neue Heartbeats

<a id="s270.b02"></a>
### S270.B02 — Vorab gültige Überlappung trägt kurze Partition

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Seiten besitzen vorab autorisierte überlappende Epochen, Partition endet innerhalb sicheren Zeitfensters und Frischeprüfung bleibt wirksam

**Warum bleibt oder endet der Zustand?** Legitime neue Heartbeats werden bis Abschlussrotation akzeptiert, nach Ablauf ohne Sync wieder Halt

**Zugeordnete Residues:** [OBR040: Begrenztes Schlüsselüberlappungsfenster](residues.md#obr040), [OBR004: Frische einer Instanz](residues.md#obr004)

**Was bleibt warum nutzbar?** Monitor kann begrenzt authentisierte frische Belege nutzen ohne alte Replays als Leben zu lesen

**Zu prüfen:** Partition über Fensterende wird trotzdem still mit altem Key akzeptiert oder frische legitime Pakete scheitern schon innerhalb gültiger Überlappung

**Architekturfolge für diesen Stressor:** Rotation als expliziten bestätigten Epochzustand mit befristeter Überlappung und Frische führen. Authfehler bedeutet unbekannt, nie grün oder unbegrenzte Altkeyannahme.

<a id="s271"></a>
## S271 — Ein zwanzig Jahre alter Vault ist lesbar aber niemand besitzt den Entschlüsselungsweg

**Ursprung:** observability; A6-Zustände: OB06, OB24. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s271.b01"></a>
### S271.B01 — Entschlüsselungsweg heute unbekannt

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Ciphertext lesbar, aber Inventar möglicher Schlüssel Escrows Hardware und Decoder ist nicht geschlossen

**Warum bleibt oder endet der Zustand?** Suche nach tatsächlich vorhandenem erlaubtem Weg kann Wait beenden, weder Fund noch endgültiges Fehlen ist vorausgesetzt

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Autorisierter Klartextzugriff ist derzeit nicht als nutzbare Struktur belegt, externe Schlüsselkopie wird nicht erfunden

**Zu prüfen:** Ein dokumentierter rechtmäßig erreichbarer Weg entschlüsselt einen repräsentativen Eintrag

<a id="s271.b02"></a>
### S271.B02 — Alle benötigten Entschlüsselungswege im Horizont verloren

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Inventar schließt alle rechtmäßig verfügbaren Schlüssel Decoder Hardware- und Rekonstruktionswege für benötigten Zeitraum aus, kein technisch machbarer alternativer Angriff im Horizont

**Warum bleibt oder endet der Zustand?** Ohne benötigte Information kein Klartextwiederaufbau, späterer kryptografischer Durchbruch würde die Zeitprämisse ändern

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für autorisierte Wiederherstellung des geschützten Inhalts bleibt nichts, lesbare Ciphertextbytes sind nicht dieser Inhalt und restliches System ist nicht automatisch verloren

**Zu prüfen:** Vorhandener erlaubter Schlüssel oder machbare Rekonstruktion entschlüsselt trotz geschlossen behauptetem Inventar

**Architekturfolge für diesen Stressor:** Key Decoder Hardware und rechtmäßig zugängliche Rekonstruktionswege samt Zeithorizont inventarisieren. Lesbarer Vault allein ist keine Klartextfähigkeit und heutige Unkenntnis kein permanenter Totalverlust.

<a id="s272"></a>
## S272 — Ein späterer kryptografischer Durchbruch macht alte sensible Exporte lesbar

**Ursprung:** observability; A6-Zustände: OB21. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s272.b01"></a>
### S272.B01 — Angreifer entschlüsselt alte Kopie

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Angreifer besitzt alte sensible Exporte, Durchbruch trifft deren tatsächliche Konstruktion und verwertbarer Klartext wird gelesen

**Warum bleibt oder endet der Zustand?** Bereits offenbarte Fakten bleiben bekannt auch wenn heutige Keys und lokale Exporte gelöscht werden

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Geheimhaltung dieser alten Fakten ist verloren, künftige Daten oder heutige Betriebsfähigkeit sind nicht dadurch vollständig verloren

**Zu prüfen:** Angreifer hatte keine Kopie oder tatsächliche Konstruktion ist unter genau dem Durchbruch weiterhin nicht entschlüsselbar

<a id="s272.b02"></a>
### S272.B02 — Durchbruch reicht für konkrete Kopie nicht nachweislich aus

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Gegenbedingung: tatsächlicher Exportbesitz verwundbare Konstruktion oder nötiges unabhängiges Schlüsselmaterial ist nicht bekannt

**Warum bleibt oder endet der Zustand?** Ohne Bestands- und Angriffsmodell bleibt Offenlegung offen, kein wohltätiger zusätzlicher Schlüssel wird als gegeben eingeführt

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für belastbare langfristige Vertraulichkeitsgarantie ist kein konkreter überlebender Schutz belegt, aber bloßes Wort Durchbruch beweist auch keine tatsächliche Kenntnis

**Zu prüfen:** Konkrete gehaltene Kopie liefert unter benannten Fähigkeiten verifizierbaren Klartext oder nachweislich unbetroffene Konstruktion wird belegt

**Architekturfolge für diesen Stressor:** Vertraulichkeitshorizont externer Kopien getrennt vom operativen Restorehorizont festlegen. Gegenwärtige Rotation oder lokale Löschung schützt bereits kopiertes verwundbares Material nicht rückwirkend.

<a id="s273"></a>
## S273 — Ein feindlicher Agent liest andere Scopes direkt über das Betriebssystem

**Ursprung:** observability; A6-Zustände: OB21, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s273.b01"></a>
### S273.B01 — Direkter OS-Lesezugriff legt fremde Daten offen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Feindlicher Agent besitzt OS-Rechte auf geschützte fremde Scopes und liest tatsächlich Klartext unter Umgehung der API

**Warum bleibt oder endet der Zustand?** Gelesene Fakten sind historisch offen, spätere API-Sperre nimmt Wissen nicht zurück

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Vertraulichkeit der tatsächlich gelesenen fremden Daten ist verloren, APIdenial oder Workspacelease ist keine Reststruktur dafür

**Zu prüfen:** OS verhindert alle entsprechenden Datei- und Prozesslesezugriffe bevor Inhalt bekannt wird

<a id="s273.b02"></a>
### S273.B02 — Effektive getrennte Principals verweigern Zugriff

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorgeschlagene OS-Isolation sperrt direkte Dateien Prozessinspektion Mounts und entsprechende Ausgänge, berechtigte eigene Arbeit bleibt bedienbar

**Warum bleibt oder endet der Zustand?** Unerlaubter direkter Zugriff endet vor Offenlegung, andere Kanäle insbesondere Timing sind gesondert zu prüfen

**Zugeordnete Residues:** [OBR039: Nicht erreichbarer Datenbereich](residues.md#obr039)

**Was bleibt warum nutzbar?** Berechtigte Nutzer können noch nicht offengelegten Datenbereich weiter verwenden während Angreifer ausgeschlossen bleibt

**Zu prüfen:** Geteilte UID offene Mounts oder Prozessrechte erlauben denselben Inhalt außerhalb Factory-API

**Architekturfolge für diesen Stressor:** Logische Scopeautorisierung von OS-Datei Prozess- und Mountrechten trennen. Historische Subprozesse unter gleichem User sind ausdrücklich kein vollständiger Sandboxnachweis.

<a id="s274"></a>
## S274 — Ein Mitnutzer rekonstruiert fremde Aktivität aus geteilten CPU- und Queuezeiten

**Ursprung:** observability; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s274.b01"></a>
### S274.B01 — Aktivität aus Timing wirklich über Priorwissen erkannt

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Angreifer beobachtet benannte CPU-/Queuezeiten, kontrollierte Auswertung erkennt sensible Opferaktivität über vorab deklarierte Priorbaseline bei akzeptierter Fehlerrate

**Warum bleibt oder endet der Zustand?** Gelernte vergangene Aktivität bleibt bekannt, fortlaufend gemeinsame Ressourcen können künftige Beobachtung ermöglichen ohne eigenen Attraktor

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Geheimhaltung tatsächlich erschlossener Aktivitätsfakten bleibt keine Restfähigkeit, kein automatischer Beweis gelesener Nutzinhalte

**Zu prüfen:** Blindes synthetisches Testset zeigt keine belastbare Verbesserung gegenüber Priorbaseline oder unerlaubte Zusatzsensoren erklären Treffer

<a id="s274.b02"></a>
### S274.B02 — Beobachtungsmodell und Signal noch unbekannt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Sichtbare Timer Angreiferkontrolle Opfergrundrate und tolerierte Fehler fehlen, gemeinsame Ressourcen allein sind gegeben

**Warum bleibt oder endet der Zustand?** Weder nützliche Inferenz noch deren Unmöglichkeit folgt ohne messbare Hypothesen

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Konkrete Timing-Vertraulichkeit ist nicht belegt, keine perfekte Abschirmung oder bereits erfolgte Offenlegung wird ergänzt

**Zu prüfen:** Vorregistriertes Angreifermodell und versteckte synthetische Aktivitätsfolge liefern unterscheidendes Ergebnis

<a id="s274.b03"></a>
### S274.B03 — Abfluss am definierten Timingkanal unterbunden

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Entwurfsalternative trennt Ressourcenzeiten oder paddet Antworten für genau deklarierte Sensoren, Kostenbudget passt und wirksame Abschirmung dieses Signals wird bedingt vorausgesetzt, der unterscheidende Nachweis steht aus

**Warum bleibt oder endet der Zustand?** Autorisierte Arbeit kann weiterlaufen während dieses Aktivitätssignal unter definierter Unterscheidungsschwelle bleibt, neue Sensoren brechen Grenze

**Zugeordnete Residues:** [OBR041: Begrenzte Timing-Abschirmung](residues.md#obr041)

**Was bleibt warum nutzbar?** Berechtigte Nutzer behalten Arbeit ohne das geprüfte Aktivitätssignal an diesen Beobachter preiszugeben, kein universeller Noninterferenz- oder empirischer Nachweis behauptet

**Zu prüfen:** Angreifer erreicht im blinden Test unter denselben erlaubten Sensoren bessere als zugesagte Unterscheidbarkeit

**Architekturfolge für diesen Stressor:** Timing als eigenen Beobachtungskanal mit Angreiferrechten Baseline und Fehlerkosten modellieren. OS-Leseverbot nicht als Nichtinterferenzbeweis wiederverwenden.

<a id="s275"></a>
## S275 — Der externe Monitor erhält aus Bequemlichkeit Shellzugriff auf Factory

**Ursprung:** observability; A6-Zustände: OB21, OB22, OB03, OB25. [Originalverläufe](../../../../reviews/observability/trajectories.csv)

<a id="s275.b01"></a>
### S275.B01 — Shellrecht vorhanden, Missbrauch nicht belegt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Monitor erhielt Shellrecht, ist aber nicht nachweislich kompromittiert und Umfang der Rechte ungeprüft

**Warum bleibt oder endet der Zustand?** Erweiterte Exposition besteht solange Rechte gelten, ohne Angreifer und Wirkung keine eindeutige Kompromissdynamik

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Unabhängige Beobachterrolle ist nicht als durchgesetzte Vertragsgrenze belegt, dennoch ist keine konkrete Datenoffenlegung behauptet

**Zu prüfen:** Inspektion belegt entweder missbrauchten wirksamen Zugriff oder tatsächlich eng begrenzte unveränderbare Nur-Lese-Autorität

<a id="s275.b02"></a>
### S275.B02 — Kompromittierte Shell legt Daten offen

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Monitor oder Shellcredential kompromittiert, Shell kann geschützte Daten lesen und Angreifer erhält sie

**Warum bleibt oder endet der Zustand?** Vergangene Offenlegung bleibt trotz spätem Widerruf, weitere Persistenz braucht zusätzliche Rechte

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Vertraulichkeit der so gelesenen Daten ist verloren, Diagnose durch denselben kompromittierten Monitor ist keine unabhängige Rettung

**Zu prüfen:** Tatsächliche Shellrechte verhindern geschützte Reads und kein unberechtigter Leser erhält Inhalt

<a id="s275.b03"></a>
### S275.B03 — Shell installiert erneut sich selbst bestätigenden Kompromiss

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Shell kann Factorypersistenz und vom Operator genutzte Monitorbelege ändern, wiederkehrende Recovery folgt diesen falschen Belegen

**Warum bleibt oder endet der Zustand?** Shell richtet Persistenz ein, manipulierte Befunde erklären Zustand gesund, gleichartige Recovery aktiviert erneut die Persistenz

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für unabhängig glaubwürdige Entscheidung innerhalb dieser kontrollierten Domäne ist kein Residue belegt, nicht jeder sonstige Byte ist verloren

**Zu prüfen:** Saubere unabhängige Recovery bleibt trotz behaupteter Rechte sauber oder manipulierte Befunde sind entscheidungsunwirksam

<a id="s275.b04"></a>
### S275.B04 — Zeugencredential bleibt technisch ohne Shellwirkung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Gegenentwurf ersetzt freie Shell durch vorab begrenzte Nur-Lese-Prüfung außerhalb gemeinsamer Administration, geschützte OS-Daten bleiben unerreichbar

**Warum bleibt oder endet der Zustand?** Kompromittierter Zeugenaufruf kann definierte Beobachtung anfragen aber nicht Factory mutieren oder Secrets lesen

**Zugeordnete Residues:** [OBR038: Anders verwalteter Nur-Lese-Zeuge](residues.md#obr038), [OBR039: Nicht erreichbarer Datenbereich](residues.md#obr039)

**Was bleibt warum nutzbar?** Operator kann begrenzte unabhängige Prüfevidenz und berechtigte Prozesse nicht offengelegte Daten nutzen, das ist andere Autorität als freie Shell

**Zu prüfen:** Zeugencredential kann doch Befehle injizieren Updater ändern oder geschützte Dateien beziehungsweise Prozesse lesen

**Architekturfolge für diesen Stressor:** Monitorcredential nicht still zum Aktuator machen. Nur-Lese-Zeuge Datenkompartiment und tatsächliche Shellmacht getrennt prüfen, bloße Rechtevergabe ist noch kein Vorfall.

<a id="s276"></a>
## S276 — Heartbeat-Labels verraten Kundennamen und vertrauliche Geschäftszeiten

**Ursprung:** boundaries; A6-Zustände: BD06. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s276.b01"></a>
### S276.B01 — Kundenwissen bereits offengelegt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Unbefugter hat Namen oder vertrauliche Zeiten tatsächlich gelernt. Eine rechtmäßig zugängliche lokale Belegkopie bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Vergangenes Lernen lässt sich nicht rückgängig machen. Weitere Verteilung kann enden.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer können Umfang der belegten Offenlegung untersuchen. Exklusivität des bereits Gelernten bleibt verloren.

**Zu prüfen:** Nachweis dass kein unbefugter Leser die Zuordnung herstellen konnte widerlegt den behaupteten Vertraulichkeitsverlust.

<a id="s276.b02"></a>
### S276.B02 — Begrenztes anonymes Lebenszeichen bleibt lesbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor Offenlegung wurde nur eine nicht rückverknüpfbare Hülle ausgegeben und Zuordnung sowie Timing sind ausreichend abgeschirmt.

**Warum bleibt oder endet der Zustand?** Technische Beobachtung läuft weiter solange diese Trennung hält. Neue Korrelation kann sie brechen.

**Zugeordnete Residues:** [BDR001: Datensparsames Lebenszeichen](residues.md#bdr001)

**Was bleibt warum nutzbar?** Berechtigter Beobachter erhält groben technischen Zustand ohne die behaupteten Kundeninformationen. Dies ist ein alternativer Entwurf nicht Rücknahme eines Lecks.

**Zu prüfen:** Synthetische Korrelation rekonstruiert Kunde oder Geschäftszeit aus Hüllen und widerlegt die Trennung.

**Architekturfolge für diesen Stressor:** Heartbeatvertrag von Kundenidentität und Betriebszeit trennen. Erhaltene technische Erreichbarkeit darf nicht als wiederhergestellte Vertraulichkeit gelten.

<a id="s277"></a>
## S277 — Ein Debugexport für Monitoring enthält die vollständige AGENTS-Konversation

**Ursprung:** boundaries; A6-Zustände: BD06, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s277.b01"></a>
### S277.B01 — Gespräch beim falschen Leserkreis

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Vollgespräch enthält geschützte Inhalte und wurde von Unbefugten gelesen. Für Untersuchung bleibt ein erlaubter metadatenarmer Beleg.

**Warum bleibt oder endet der Zustand?** Historische Offenlegung bleibt auch nach Löschung beim Exportdienst.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Der Vorfallsprüfer kann den dokumentierten Transfer abgrenzen. Der Beleg ist keine Rückgewinnung der Geheimhaltung und enthält keine zusätzliche Gesprächskopie.

**Zu prüfen:** Kein unbefugter Zugang oder kein geschützter Inhalt widerlegt diesen Verlustzweig.

<a id="s277.b02"></a>
### S277.B02 — Volltext zurückgehalten Diagnose nutzbar

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängige lokale Feldauswahl hält Volltext vor Grenzübertritt zurück und Support darf den reduzierten Auszug lesen.

**Warum bleibt oder endet der Zustand?** Der Volltextexport bleibt gesperrt bis eine zulässige engere Auswahl feststeht.

**Zugeordnete Residues:** [BDR002: Begrenzter Diagnoseauszug](residues.md#bdr002)

**Was bleibt warum nutzbar?** Support kann erlaubte Fehlerparameter nutzen ohne die ganze AGENTS-Konversation. Die Sperre betrifft nur diesen Export.

**Zu prüfen:** Ein Geheimnismarker gelangt über Freitext in den Auszug oder Diagnose ist ohne Volltext unmöglich.

**Architekturfolge für diesen Stressor:** Diagnoseauszug positiv spezifizieren und vor Export abnehmen. Bereits gelernte Konversation kann kein Löschknopf wieder geheim machen.

<a id="s278"></a>
## S278 — Ein Angreifer löst gezielt Fehlalarme aus um Alarmempfänger und Bereitschaftsmuster zu lernen

**Ursprung:** boundaries; A6-Zustände: BD07, BD06, BD18. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s278.b01"></a>
### S278.B01 — Fortgesetztes Ausforschen der Bereitschaft

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Angreifer kann wiederholt auslösen und Antworten korrelieren. Getrennt gespeicherte Beobachtungen bleiben rechtmäßig lesbar.

**Warum bleibt oder endet der Zustand?** Probe führt zu Antwort und gezielterer Folgeprobe. Fortdauer benötigt den aktiven Angreifer.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Die Untersuchung kann beobachtete Probeantworten vergleichen. Sie verhindert weder weitere Inferenz noch macht sie gelerntes Wissen geheim.

**Zu prüfen:** Entfernen der unterscheidbaren Antworten beseitigt Korrelation obwohl Proberate gleich bleibt.

<a id="s278.b02"></a>
### S278.B02 — Probe verrät keinen Empfänger mehr

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Nicht identifizierende Hülle und vom realen Bereitschaftsmuster entkoppelte Antworten werden tatsächlich vor Ausgabe durchgesetzt.

**Warum bleibt oder endet der Zustand?** Anfrage darf keine unterscheidbare Empfängerinformation ausgeben. Ende der Regel öffnet die Grenze erneut.

**Zugeordnete Residues:** [BDR001: Datensparsames Lebenszeichen](residues.md#bdr001)

**Was bleibt warum nutzbar?** Der legitime Beobachter kann grobe Health weiter lesen während der externe Probeeffekt begrenzt ist. Andere Timingkanäle sind nicht mitbewiesen.

**Zu prüfen:** Angreifer lernt mit synthetischen Proben dennoch reale Empfänger oder Arbeitszeiten.

<a id="s278.b03"></a>
### S278.B03 — Aufmerksamkeit reproduziert ihren Rückstand

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Auch nach Ende der Proben erzeugen ungelöste Vorfälle Erinnerungen oberhalb der Bearbeitungsrate. Ein getrenntes Vorfallsregister bleibt erreichbar.

**Warum bleibt oder endet der Zustand?** Rückstand bindet Reparaturzeit → mehr ungelöste Vorfälle → neue Erinnerungen → mehr Rückstand bei fester Grundlast.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Einsatzplanung kann einzelne offene Vorfälle weiterfinden. Das Register verspricht weder rechtzeitige Reparatur noch unendliche Speicherung.

**Zu prüfen:** Nach Ende des Angriffs bei unveränderter Grundlast und Personalstärke sinkt der Rückstand ohne neue innere Produktion bis null.

**Architekturfolge für diesen Stressor:** Proberückkanal und Alarmzustellung getrennt begrenzen. Weder Rate-Limit noch eine endliche Nachricht erhalten automatisch menschliche Aufmerksamkeit für alle Vorfälle.

<a id="s279"></a>
## S279 — Ein fehlerhafter Exporter erzeugt identisch falsche Daten und passende Prüfsummen in allen Backups

**Ursprung:** boundaries; A6-Zustände: BD04, BD05, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s279.b01"></a>
### S279.B01 — Ein falscher Export einmal übernommen

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Ein fehlerhafter Export wird einmal restauriert ohne folgende Vertrauensverstärkung. Ein separater autorisierter Mengenursprung existiert noch.

**Warum bleibt oder endet der Zustand?** Der falsche Import endet als einzelnes falsches Ergebnis. Abgleich kann neue Korrekturarbeit auslösen.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Fachprüfer können betroffene Mengen gegen den nicht vom Exporter erzeugten Ursprung prüfen. Signaturen der Kopien leisten das nicht.

**Zu prüfen:** Auch der angeblich unabhängige Ursprung wurde vom selben Exporter abgeleitet oder stimmt nach unabhängigem Abgleich korrekt überein.

<a id="s279.b02"></a>
### S279.B02 — Kopien bestätigen fortgesetzt dieselbe falsche Wahrheit

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Restaurierter Fehler wird wieder kanonischer Ursprung und alle tatsächlichen Vergleiche stammen davon. Kein unabhängiger Ursprung ist zugänglich.

**Warum bleibt oder endet der Zustand?** Falsche Daten → passende Checks → mehr Vertrauen → Überschreiben von Alternativen → erneute falsche Daten.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Wiedergewinnung der ursprünglichen Fachwahrheit gibt es in diesem geschlossenen Bestand keine nutzbare Struktur. Internes Rechnen bleibt möglich aber ist kein Wahrheitsresidue.

**Zu prüfen:** Eine rechtmäßig zugängliche unabhängige Ursprungsquelle oder ausbleibende Wiederverwendung widerlegt jeweils Informationsgrenze oder Schleife.

<a id="s279.b03"></a>
### S279.B03 — Unabhängiger Ursprung stoppt Übernahme

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor Restaurierung vergleichen berechtigte Prüfer Daten gegen tatsächlich erhaltenen anderen Ursprung und verhindern Adoption bei Abweichung.

**Warum bleibt oder endet der Zustand?** Import wartet auf Klärung oder wird aufgegeben. Kein autonomer Attraktor.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Der Ursprungsbetrag bleibt zur Ablehnung des konsistent falschen Imports nutzbar. Andere nicht unabhängig belegte Felder bleiben ungesichert.

**Zu prüfen:** Ein gültiger aber falscher Betrag passiert den Abgleich oder Prüfer kann Adoption nicht verhindern.

**Architekturfolge für diesen Stressor:** Exportprüfsumme von unabhängiger Fachwahrheit trennen. Ohne überlebenden Ursprung keine korrekte Rekonstruktion behaupten.

<a id="s280"></a>
## S280 — Ein einzelner Bitfehler verwandelt einen Betrag in eine andere noch gültige Zahl

**Ursprung:** boundaries; A6-Zustände: BD04, BD10, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s280.b01"></a>
### S280.B01 — Bitfehler vor Nutzung erkannt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Anderer überlebender Mengenbeleg widerspricht dem gültigen geänderten Wert und Wirkung ist noch kontrollierbar.

**Warum bleibt oder endet der Zustand?** Betroffene Zahlung wartet bis der Betrag kompetent geklärt wird.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Autorisierter Bearbeiter kann ursprüngliche Menge vergleichen und diesen neuen Fehler zurückhalten.

**Zu prüfen:** Ein einzelner gültiger Bitflip passiert den unabhängigen Mengenabgleich.

<a id="s280.b02"></a>
### S280.B02 — Falscher Betrag bereits wirksam

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Geänderter Betrag wurde angenommen. Ein getrennt erhaltener ursprünglicher Mengenbeleg bleibt zugänglich.

**Warum bleibt oder endet der Zustand?** Einmalige falsche Zahlung ist kein Regelkreis. Ausgleich ist neue Arbeit und kann den Saldo reparieren.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Prüfer kann den falschen Betrag quantifizieren und Ausgleich begründen. Bereits versandte Information oder andere irreversible Folge bleibt ggf. verloren.

**Zu prüfen:** Wirkungsbeleg zeigt den korrekten ursprünglichen Betrag oder angeblicher Mengenursprung ist mitverändert.

<a id="s280.b03"></a>
### S280.B03 — Ursprungsbetrag nicht mehr bestimmbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Nur geänderter gültiger Wert blieb übrig und keine erforderliche ursprüngliche Zahl ist zugänglich.

**Warum bleibt oder endet der Zustand?** Ohne neue unterscheidende Information bleibt der wahre Betrag offen. Kein Schluss auf Totalverlust.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die genaue ursprüngliche Menge fehlt jeder tragfähige Vergleich. Schema und Prüfsumme liefern die verlorene Fachinformation nicht.

**Zu prüfen:** Ein unabhängiger rechtmäßig zugänglicher Mengenbeleg würde diese Informationsannahme widerlegen.

**Architekturfolge für diesen Stressor:** Betragsherkunft vor fachlicher Wirkung prüfen. Eine gültige Zahl und eine spätere finanzielle Korrektur beweisen weder Originalbetrag noch ungeschehenen Vorgang.

<a id="s281"></a>
## S281 — Zwei signierte Journale widersprechen sich nach einer Split-Brain-Phase

**Ursprung:** boundaries; A6-Zustände: BD27, BD08, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s281.b01"></a>
### S281.B01 — Zwei authentische Geschichten bleiben unentschieden

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Journale sind erhalten aber keine belastbare Autoritätsordnung oder Teilnehmergeschichte entscheidet den Streit.

**Warum bleibt oder endet der Zustand?** Signaturen wiederholen Herkunft und lösen den Widerspruch nicht. Neue unabhängige Evidenz kann ihn klären.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Legitimer Prüfer kann beide Geschichten samt Lücken lesen ohne einen Sieger zu erfinden.

**Zu prüfen:** Eine durchgesetzte zeitliche Autoritätsordnung erklärt alle scheinbar widersprechenden Einträge.

<a id="s281.b02"></a>
### S281.B02 — Gegenseitige Reparaturen halten Split-Brain am Leben

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Beide Writer bleiben wirksam und reagieren auf Änderungen des anderen. Unabhängige Belegkopien bleiben lesbar.

**Warum bleibt oder endet der Zustand?** A ändert → B repariert nach B → A repariert nach A. Die anfängliche Netzstörung muss nicht fortbestehen.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer können Konfliktverlauf rekonstruieren soweit ihre Kopien reichen. Belege selbst stellen keine Exklusivität her.

**Zu prüfen:** Einer wird am Teilnehmer tatsächlich abgewiesen oder beide hören ohne Fremdeingriff auf gegeneinander zu reparieren.

<a id="s281.b03"></a>
### S281.B03 — Ein Writer ausgeschlossen frühere Effekte geklärt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Jeder relevante Teilnehmer erzwingt neue Autorität und vollständige unabhängige Wirkungsbelege klären den benannten Konflikt.

**Warum bleibt oder endet der Zustand?** Die einzelne Übergabe und ihr Abgleich sind abgeschlossen. Neue Arbeit braucht neue Prüfung.

**Zugeordnete Residues:** [BDR008: Teilnehmerseitige Schreibgeneration](residues.md#bdr008), [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Neuer Berechtigter kann exklusiv schreiben und Prüfer den begrenzten Abgleich nachvollziehen. Frühere reale Doppelwirkung wird nicht ungeschehen.

**Zu prüfen:** Ein gepufferter Altauftrag wird angenommen oder ein beteiligter Effekt fehlt im behaupteten Abgleich.

**Architekturfolge für diesen Stressor:** Signierte Aussagen mit kausaler Autorität und Wirkungsbelegen verbinden. Konfliktaufbewahrung und Teilnehmerausschluss sind getrennte Verträge.

<a id="s282"></a>
## S282 — Ein altes Backup kennt weder gestern versandte Rechnungen noch heutige Widerrufe

**Ursprung:** boundaries; A6-Zustände: BD03, BD09, BD10, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s282.b01"></a>
### S282.B01 — Erhaltene Versuche bleiben offen gesperrt

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Snapshot enthält bekannte Versuche und lokale Wiederholkontrolle bleibt wirksam. Ausgang lässt sich noch nicht feststellen.

**Warum bleibt oder endet der Zustand?** Keine Antwort ist kein Fehlschlag. Wiederholsperre dauert bis Abgleich oder gesonderter Risikoentscheidung.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Operator kann nur die erhaltenen Versuche untersuchen ohne automatische Wiederholung. Gestern fehlende Rechnungen sind ausdrücklich nicht mit erfasst.

**Zu prüfen:** Ein im Snapshot enthaltener unklarer Versuch wird automatisch wieder gesendet oder fehlender Nachlauf als vollständig ausgegeben.

<a id="s282.b02"></a>
### S282.B02 — Widerruf erreicht letzte Annahmegrenze

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Aktuelle Widerrufsquelle und Teilnehmerprüfung überleben unabhängig vom alten Backup und sehen alle betroffenen Annahmen.

**Warum bleibt oder endet der Zustand?** Teilnehmer verweigert alte Rechte bis eine neue legitime Freigabe gilt.

**Zugeordnete Residues:** [BDR010: Annahmegebundene Gültigkeit](residues.md#bdr010)

**Was bleibt warum nutzbar?** Der Teilnehmer kann heutige Widerrufe trotz altem lokalen Stand anwenden. Dies beantwortet nicht ob gestern schon Rechnungen versandt wurden.

**Zu prüfen:** Gepufferter Auftrag wird unter einem heute wirksamen Widerruf dennoch angenommen.

<a id="s282.b03"></a>
### S282.B03 — Fehlender Rechnungslauf bleibt nicht rekonstruierbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Snapshot enthält den gestrigen Lauf nicht und kein rechtmäßig zugänglicher Teilnehmer- oder Empfängerbeleg existiert.

**Warum bleibt oder endet der Zustand?** Outcome bleibt offen solange erforderliche Nachlaufdaten fehlen. Neue Freigabe schließt diese Lücke nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für den tatsächlichen gestrigen Versand fehlt ein nutzbarer Nachweis. Unbetroffene lokale Daten können weiterhin bestehen.

**Zu prüfen:** Ein korrelierter unabhängiger Versandbeleg widerlegt die angenommene Nachweislücke.

**Architekturfolge für diesen Stressor:** Snapshotalter und fehlenden Nachlauf sichtbar halten. Restore-Sperre nur auf tatsächlich enthaltene Versuche beziehen und aktuelle Teilnehmerrechte gesondert prüfen.

<a id="s283"></a>
## S283 — Original und restaurierter Klon senden gleichzeitig plausible Lebenszeichen

**Ursprung:** boundaries; A6-Zustände: BD27, BD08, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s283.b01"></a>
### S283.B01 — Zwei plausible Lebenszeichen ohne eindeutigen Eigentümer

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Original und Klon leben und ihre getrennten Beobachtungen werden erhalten. Keine Annahmeordnung ist belegt.

**Warum bleibt oder endet der Zustand?** Beobachtung kann fortdauern ohne zu entscheiden wer schreiben darf.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer können zwei Identitätsbehauptungen nebeneinander halten. Health liefert keinen exklusiven Writerbeweis.

**Zu prüfen:** Durchgesetzte Teilnehmergeneration bestimmt eindeutig den aktiven Writer.

<a id="s283.b02"></a>
### S283.B02 — Zwei Reparaturschleifen konkurrieren

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Beide besitzen Außenrechte und jeder gleicht Drift zur eigenen Sicht aus. Getrennte Belegkopien bleiben lesbar.

**Warum bleibt oder endet der Zustand?** Klonänderung → Originalkorrektur → Klonkorrektur reproduziert Konflikt nach Ende des Restoreereignisses.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Unabhängiger Prüfer behält die dokumentierte Konfliktspur aber keine Exklusivität und keine Garantie vollständiger Wirkungsgeschichte.

**Zu prüfen:** Nur ein Writer ist effektiv oder keine gegenseitige Reaktion folgt bei unveränderter Grundlast.

<a id="s283.b03"></a>
### S283.B03 — Klonfortsetzung mit wirksam ausgeschlossenem Original

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Dauerhafte legitime Generation ist am letzten Teilnehmer wirksam. Alte Wirkungen werden separat und noch nicht vollständig abgeglichen.

**Warum bleibt oder endet der Zustand?** Neue Schreibkonkurrenz endet mit Ablehnung des Originals. Alter Outcome kann weiter offen bleiben.

**Zugeordnete Residues:** [BDR008: Teilnehmerseitige Schreibgeneration](residues.md#bdr008)

**Was bleibt warum nutzbar?** Neuer Writer kann begrenzt exklusiv fortfahren. Diese Fähigkeit ist nicht gleich Abschluss aller vorigen Aktionen.

**Zu prüfen:** Originalauftrag einschließlich früher gepufferter Kopie bewirkt nach Generationstransfer noch einen neuen Effekt.

**Architekturfolge für diesen Stressor:** Heartbeatidentität nicht mit exklusiver Wirkmacht gleichsetzen. Generation muss bei allen Effektteilnehmern greifen einschließlich alter Puffer.

<a id="s284"></a>
## S284 — Der Recoverylauf dauert so lange dass seine zu Beginn geprüften Rechte schon wieder verfallen

**Ursprung:** boundaries; A6-Zustände: BD09, BD02, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s284.b01"></a>
### S284.B01 — Recoveryfertig aber Auftrag abgelaufen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Der Teilnehmer prüft aktuelle Rechte nach langem Restore und verweigert abgelaufene Mandate vor Wirkung.

**Warum bleibt oder endet der Zustand?** Die konkrete Aktion bleibt bis frischer legitimer Autorität zurückgehalten.

**Zugeordnete Residues:** [BDR010: Annahmegebundene Gültigkeit](residues.md#bdr010)

**Was bleibt warum nutzbar?** Teilnehmer kann historische Rechte als nicht mehr ausreichend erkennen obwohl Recovery lokal erfolgreich ist.

**Zu prüfen:** Nach Ablauf nimmt Teilnehmer einen alten Recoveryauftrag an.

<a id="s284.b02"></a>
### S284.B02 — Alte Rechte möglicherweise schon benutzt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur Startprüfung existiert und Auftrag kann bereits angenommen sein. Erhaltener Versuchseintrag und lokale Wiederholsperre bestehen.

**Warum bleibt oder endet der Zustand?** Mögliche späte Wirkung bleibt offen bis unabhängiger Annahmebeleg vorliegt.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Operator kann den bekannten Versuch ohne blindes Retry untersuchen. Die Sperre verhindert seine alte Außenannahme nicht.

**Zu prüfen:** Vollständiger Annahmebeleg zeigt Ablehnung oder der behauptete bekannte Versuch fehlt im erhaltenen Register.

**Architekturfolge für diesen Stressor:** Recoveryfortschritt darf keine neue Autorität erzeugen. Gültigkeit beim letzten Effekt prüfen und fehlende Rückmeldung separat halten.

<a id="s285"></a>
## S285 — Neuer Client und altes Plugin interpretieren dieselbe gültige Zahl mit verschiedener Einheit

**Ursprung:** boundaries; A6-Zustände: BD04, BD02, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s285.b01"></a>
### S285.B01 — Inkompatible Einheit vor Wirkung verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Client und Plugin tauschen konkrete Dimensionsverträge aus und ein zuständiger Prüfer erkennt die Abweichung vor Mutation.

**Warum bleibt oder endet der Zustand?** Zusammenarbeit wartet auf passenden Vertrag oder endet ohne Aktion.

**Zugeordnete Residues:** [BDR005: Dimensionsgebundener Wertevertrag](residues.md#bdr005)

**Was bleibt warum nutzbar?** Beide Seiten können die jeweils gemeinte Menge feststellen und diese inkompatible Verwendung verweigern.

**Zu prüfen:** Eine gültige Zahl wird ohne vereinbarte Einheit trotzdem zur Außenaktion.

<a id="s285.b02"></a>
### S285.B02 — Gültige Zahl erzeugt falsche Menge

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Plugin interpretiert Zahl in falscher Einheit und der Effekt ist erfolgt. Originaler unabhängiger Mengenbeleg bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Einmalige Fehlinterpretation bleibt falscher Abschluss. Irreversibler Anteil wäre gesonderter Verlust.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Fachprüfer kann Abweichung quantifizieren. Die erhaltene Menge korrigiert den vergangenen Effekt nicht automatisch.

**Zu prüfen:** Unabhängiger Annahmebeleg weist die tatsächlich beabsichtigte Menge nach.

**Architekturfolge für diesen Stressor:** Bedeutungsvertrag mit Einheit Skalierung und Version an die Wirkung binden statt nur JSON-Zahlen zu validieren.

<a id="s286"></a>
## S286 — Ein optionales Plugin verlangt plötzlich eine globale Kernmigration für alle Scopes

**Ursprung:** boundaries; A6-Zustände: BD02, BD15, BD12. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s286.b01"></a>
### S286.B01 — Globale Forderung lokal abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kern lässt optionale Pluginanforderung keine globale Migration erzwingen und andere Scopes brauchen dessen Code nicht.

**Warum bleibt oder endet der Zustand?** Plugin bleibt deaktiviert bis kompatible Lösung vorliegt. Andere Scopes arbeiten im abgegrenzten lokalen Umfang.

**Zugeordnete Residues:** [BDR012: Scopebegrenzte Startfähigkeit](residues.md#bdr012)

**Was bleibt warum nutzbar?** Nicht aktivierende Scopenutzer behalten ihren lokalen Start und Datenzugriff. Kein Beweis gegen gemeinsame Hostausfälle.

**Zu prüfen:** Nicht aktivierender Scope wartet trotzdem auf Migration oder Cloud des Plugins.

<a id="s286.b02"></a>
### S286.B02 — Alle Startpfade warten auf Kernmigration

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Globale Startabhängigkeit wurde tatsächlich eingebaut. Lokal lesbare Daten und Notreader liegen außerhalb dieser Abhängigkeit.

**Warum bleibt oder endet der Zustand?** Solange Pflichtmigration fehlt bleibt regulärer Start blockiert. Abkopplung kann ihn wieder ermöglichen.

**Zugeordnete Residues:** [BDR011: Unabhängig lesbarer Notstart](residues.md#bdr011), [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Lokaler Betreiber kann vorhandenen Bestand und Konfiguration untersuchen ohne betroffenen Pluginstart. Produktbetrieb ist nicht erhalten.

**Zu prüfen:** Notreader ruft das Plugin auf oder Datenzugriff verlangt genau denselben Startpfad.

<a id="s286.b03"></a>
### S286.B03 — Zirkulärer Migrationsnotstart ohne anderen Leser

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Reparatur braucht Pluginkonfiguration und nur das defekte Plugin kann sie lesen. Keine unabhängigen Schlüssel oder Reader sind verfügbar.

**Warum bleibt oder endet der Zustand?** Readerreparatur wartet auf Konfiguration und Konfigurationszugriff auf Readerreparatur.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für in-band Reparatur fehlt ein nutzbarer Pfad. Rohe Bytes oder unbetroffene Funktionen begründen keinen lesbaren Notstart.

**Zu prüfen:** Ein unabhängiger kompatibler und berechtigter Reader liest die nötige Konfiguration.

**Architekturfolge für diesen Stressor:** Kernmigration vom optionalen Pluginvertrag trennen. Scopeisolation nur für unabhängig startbare Pfade behaupten und Notreader gesondert sichern.

<a id="s287"></a>
## S287 — Altes Binary öffnet einen neuen Store und ersetzt unverstandene Felder durch Defaults

**Ursprung:** boundaries; A6-Zustände: BD02, BD04, BD05. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s287.b01"></a>
### S287.B01 — Altes Binary darf nur unverändert lesen oder ablehnen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Versionsprüfung vor Mutation greift und der neue Bestand bleibt vollständig lesbar für passenden Nachfolger.

**Warum bleibt oder endet der Zustand?** Alter Writer bleibt ausgeschlossen bis kompatibler Code verfügbar ist.

**Zugeordnete Residues:** [BDR013: Unverändert lesbarer Versionsbestand](residues.md#bdr013)

**Was bleibt warum nutzbar?** Späterer kompatibler Leser erhält originale unbekannte Felder. Das ist Byteerhalt nicht garantierte fachliche Richtigkeit.

**Zu prüfen:** Ein unbekanntes Feld verändert sich schon beim Öffnen mit altem Binary.

<a id="s287.b02"></a>
### S287.B02 — Unbekannte einzige Felder durch Defaults vernichtet

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Altes Binary überschreibt einzigartige Werte und keine weitere Kopie oder Bedeutungsquelle enthält sie.

**Warum bleibt oder endet der Zustand?** Exakte alte Werte bleiben aus Defaults nicht rekonstruierbar. Andere Felder können nutzbar bleiben.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die überschriebenen Werte gibt es kein Residue. Schemavalidität enthält die entfernte Information nicht.

**Zu prüfen:** Unveränderte rechtmäßig lesbare Kopie oder eindeutige Ursprungsquelle erlaubt genaue Rekonstruktion.

<a id="s287.b03"></a>
### S287.B03 — Defaults werden zur selbstbestätigten Wahrheit

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Defaultbestand wird als Ursprung aller Checks und weiteren Backups wiederverwendet und Alternativen verdrängt.

**Warum bleibt oder endet der Zustand?** Defaults → passende Prüfberichte → Vertrauen → weitere Defaultkopien → weniger Vergleichsmöglichkeiten.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für den ursprünglichen unbekannten Feldinhalt fehlt im geschlossenen Bestand jede belastbare Rekonstruktionsfähigkeit. Wiederholbare falsche Berechnung bleibt keine Wahrheit.

**Zu prüfen:** Unabhängige Ursprungswerte beeinflussen Entscheidungen oder kein wiederholtes Vertrauen reproduziert die Übernahme.

**Architekturfolge für diesen Stressor:** Vor erstem Schreibzugriff Writerkompatibilität prüfen. Unbekannte Felder dürfen nicht normalisiert werden und verlorene einzige Werte bleiben verloren.

<a id="s288"></a>
## S288 — Rollback startet den alten Code nachdem der neue schon eine irreversible Außenaktion ausgelöst hat

**Ursprung:** boundaries; A6-Zustände: BD10, BD03. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s288.b01"></a>
### S288.B01 — Irreversible Außenwirkung bleibt trotz Rollback

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Die Wirkung fand statt und der benannte Anteil etwa gelernte Information ist irreversibel. Separater zulässiger Wirkungsbeleg überlebt den Coderollback.

**Warum bleibt oder endet der Zustand?** Vergangene Wirkung bleibt historisch bestehen. Späterer Ausgleich kann nur spätere Lage ändern.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann belegten Vorgang nachvollziehen. Fähigkeit ihn ungeschehen zu machen besteht nicht.

**Zu prüfen:** Es gab keine Wirkung oder die behauptete verlorene Eigenschaft ist vollständig rückstellbar.

<a id="s288.b02"></a>
### S288.B02 — Alter Code wartet auf Ausgangsabgleich

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorwärts kompatibel erhaltenes Versuchsregister enthält neuen Effektversuch und bindet auch den zurückgerollten Code an Wiederholsperre.

**Warum bleibt oder endet der Zustand?** Unklarer Ausgang bleibt bis korreliertem Wirkungsbeleg offen.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Berechtigter Operator behält bekannte Operationsidentität und unterlässt blindes erneutes Senden. Weltrollback wird nicht versprochen.

**Zu prüfen:** Altes Binary ignoriert Register oder behandelt neuen unbekannten Status als nie versucht.

**Architekturfolge für diesen Stressor:** Rollback mit vorwärts erhaltenem Wirkungsregister verbinden. Lokal alter Code darf weder Außenwelt rückdatieren noch fehlende neue Effekte als ungesendet behandeln.

<a id="s289"></a>
## S289 — Eine Migration verliert die Verbindung genau zwischen Übergabe und Start des neuen Writers

**Ursprung:** boundaries; A6-Zustände: BD11, BD08, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s289.b01"></a>
### S289.B01 — Übergabestand unbekannt beide warten

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Es gibt zwei potenzielle Writer und verlorene Antwort. Beide verweigern neue Writes und erhalten Übergabefragmente.

**Warum bleibt oder endet der Zustand?** Fehlende Eigentümerevidenz hält Schreibpause aufrecht bis legitime Autorität eindeutig wird.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Zuständiger Prüfer kann Fragmente vergleichen ohne Eigentümer zu raten. Lokale Transaktionsatomizität ersetzt diesen Beleg nicht.

**Zu prüfen:** Durabler Eigentümernachweis entscheidet den Übergang bereits oder einer schreibt trotz behaupteter Pause.

<a id="s289.b02"></a>
### S289.B02 — Durabler Transfer erlaubt einen Nachfolger

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle betroffenen Teilnehmer erkennen dauerhaft die neue Generation an und vorherige Übergabeschritte sind vollständig geklärt.

**Warum bleibt oder endet der Zustand?** Dieser Writerwechsel ist abgeschlossen auch wenn Antworttransport zeitweise ausfällt.

**Zugeordnete Residues:** [BDR008: Teilnehmerseitige Schreibgeneration](residues.md#bdr008)

**Was bleibt warum nutzbar?** Legitimer Nachfolger kann exklusiv schreiben. Andere Businessoutcomes brauchen weiterhin eigene Belege.

**Zu prüfen:** Ein Crashpunkt erlaubt zwei effektive Writer oder alter gepufferter Write wird akzeptiert.

<a id="s289.b03"></a>
### S289.B03 — Geratene Übergabe erzeugt Reparaturkreislauf

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Beide starten mit wirksamen Rechten und korrigieren gegenseitig ihre Änderungen. Unabhängige begrenzte Belegkopie bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Unklare Übergabe → konkurrierende Writes → Gegenreparaturen → neue Gegenreparaturen nach wiederhergestellter Verbindung.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann Konfliktbeginn und erhaltene Wiederholungen sehen. Belegaufbewahrung stoppt keinen der Writer.

**Zu prüfen:** Ohne weitere Verbindungsstörung und bei gleicher Grundlast hört Konflikt auf weil einer effektiv ausgeschlossen ist.

**Architekturfolge für diesen Stressor:** Verteilte Writerübergabe von lokaler SQL-Migration unterscheiden. Dauerhafte Übergabeevidenz und echte Teilnehmerexklusion vor Start des Nachfolgers benötigen getrennte Nachweise.

<a id="s290"></a>
## S290 — Eine Notfallkorrektur benötigt gerade das kaputte Plugin um dessen Konfiguration zu lesen

**Ursprung:** boundaries; A6-Zustände: BD12, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s290.b01"></a>
### S290.B01 — Reparatur im Kreis blockiert

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Die einzige Konfigurationsinterpretation läuft im defekten Plugin und es gibt keinen nutzbaren unabhängigen Reader.

**Warum bleibt oder endet der Zustand?** Reparatur wartet auf Konfiguration und Konfiguration auf reparierten Leser.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** In-band Konfigurationsreparatur hat keine nutzbare Struktur. Rohdatei ist ohne nötige Semantik und Schlüssel keine Reparaturfähigkeit.

**Zu prüfen:** Ein legal nutzbarer unabhängiger Reader interpretiert die benötigten Felder.

<a id="s290.b02"></a>
### S290.B02 — Lokaler Notreader erlaubt vorbereitete Korrektur

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Spezifikation Reader lokale Bytes und nötige Berechtigung liegen außerhalb des Plugins. Neue Mutation wartet auf Prüfung.

**Warum bleibt oder endet der Zustand?** Leseblockade endet. Konfigurationsänderung bleibt bis kompetenter Freigabe gehalten.

**Zugeordnete Residues:** [BDR011: Unabhängig lesbarer Notstart](residues.md#bdr011)

**Was bleibt warum nutzbar?** Lokaler Betreiber kann Fehlerkonfiguration prüfen ohne den defekten Teil aufzurufen. Reader erteilt keine Wirkungsvollmacht.

**Zu prüfen:** Notreader muss zum Entschlüsseln oder Deuten das kaputte Plugin starten.

**Architekturfolge für diesen Stressor:** Notfallkonfiguration ohne Start des kaputten Plugins interpretierbar machen. Schlüsselabhängigkeit und Außenfreigabe bleiben eigenständige Grenzen.

<a id="s291"></a>
## S291 — Der einzige Maintainer verschwindet und ein Nachfolger kennt die verdeckten Betriebsannahmen nicht

**Ursprung:** boundaries; A6-Zustände: BD13, BD14. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s291.b01"></a>
### S291.B01 — Nachfolger wartet gewöhnlich auf Betriebswissen

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Keine wiederholte Patch- oder Ausnahmeverstärkung ist belegt. Ein separat erhaltenes Belegpaket bisheriger Betriebsaussagen mit Herkunft und dokumentierten Lücken ist dem legitimierten Nachfolger zugänglich aber erforderliche Annahmen fehlen.

**Warum bleibt oder endet der Zustand?** Wartezeit endet wenn fehlende Annahmen geklärt sind. Ohne Klärung bleibt Fähigkeit zur korrekten Reparatur offen.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Nachfolger kann dokumentierte Aussagen und Wissenslücken prüfen. Das Belegpaket enthält keine vollständige Bauumgebung und beweist keine kompetente Reparatur.

**Zu prüfen:** Nachfolger kann allein aus bestehender Dokumentation den konkreten Incident korrekt beheben.

<a id="s291.b02"></a>
### S291.B02 — Incidents verhindern Lernen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Incidentarbeit verbraucht Lernzeit und jeder schnelle Patch vermehrt verborgene Annahmen. Separat erhaltene Betriebsaussagen samt Herkunft und Lücken bleiben legitim zugänglich.

**Warum bleibt oder endet der Zustand?** Wissensmangel → teure Reparatur → weniger Lernen und mehr verdeckte Patches → größerer Wissensmangel.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Nachfolger kann dokumentierte Behauptungen und Lücken prüfen aber daraus folgt keine reproduzierbare Wartung. Verstärkung ist eine soziale Hypothese.

**Zu prüfen:** Bei fester Incidentrate baut Nachfolger Wissen auf und reduziert Patchkosten ohne neue verdeckte Annahmen.

<a id="s291.b03"></a>
### S291.B03 — Nachvollziehbare Übergabe ermöglicht begrenzte Wartung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Erhaltenes getestetes Bau- und Betriebspaket sowie legitime Rechte decken die konkret benötigte Reparatur ab.

**Warum bleibt oder endet der Zustand?** Übergabelücke endet mit demonstriertem unabhängigen Betrieb. Neue unbekannte Fälle bleiben außerhalb des Nachweises.

**Zugeordnete Residues:** [BDR014: Nachvollziehbares Betriebs- und Baupaket](residues.md#bdr014)

**Was bleibt warum nutzbar?** Nachfolger kann diese Reparatur ohne verschwundenen Maintainer erklären bauen und prüfen.

**Zu prüfen:** Ein benötigter Schritt verlangt verborgenes Wissen oder nicht erhältlichen Accountzugriff.

**Architekturfolge für diesen Stressor:** Verdeckte Annahmen als begrenztes Übergabepaket explizit machen. Weggang allein begründet weder Wartungsattraktor noch erloschene Reparaturbefugnis.

<a id="s292"></a>
## S292 — Eine winzige Reparatur braucht eine nicht mehr erhältliche Toolchain und einen gelöschten Cloudaccount

**Ursprung:** boundaries; A6-Zustände: BD37, BD13, BD22. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s292.b01"></a>
### S292.B01 — Reparatur wartet auf verschwundene Voraussetzung

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Toolchain und Cloudkonto fehlen derzeit aber vorhandenes Binary und lokale interpretierbare Daten sind weiterhin zugänglich.

**Warum bleibt oder endet der Zustand?** Fehlende Voraussetzung hält Reparatur auf. Ein legitimer kompatibler Ersatz kann den Zustand beenden ohne dass vorher ein Kreis bestand.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Berechtigter Betreiber kann erhaltene Geschäftsdaten prüfen. Weder vorhandene Quellen noch Daten ersetzen Konto und Compiler.

**Zu prüfen:** Reparatur gelingt mit ausschließlich aktuell vorhandenen unabhängigen Eingaben.

<a id="s292.b02"></a>
### S292.B02 — Lokaler Ersatzbau überlebt Kontoverlust

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vollständiges verfügbares Baupaket enthält kompatible Werkzeuge und der konkrete Fix braucht den gelöschten Account nicht.

**Warum bleibt oder endet der Zustand?** Die konkrete Reparatur kann nach unabhängigem Vergleich fertiggestellt werden. Fremde Providerfunktion bleibt getrennt.

**Zugeordnete Residues:** [BDR014: Nachvollziehbares Betriebs- und Baupaket](residues.md#bdr014)

**Was bleibt warum nutzbar?** Nachfolger kann den kleinen Fix reproduzieren und prüfen. Eine versteckte Cloudbauabhängigkeit würde diesen Kandidaten aufheben.

**Zu prüfen:** Offlinebau verlangt den gelöschten Account oder erzeugt semantisch inkompatibles Binary.

<a id="s292.b03"></a>
### S292.B03 — Providergebundene Fähigkeit endet ohne Ersatz

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Die benötigte Konto- oder Providerfunktion ist endgültig entfallen und kein derzeit legitim nutzbarer Ersatz existiert.

**Warum bleibt oder endet der Zustand?** Diese Funktion bleibt beendet bis eine neue tatsächlich brauchbare Alternative entsteht. Kein allweltlicher Dauerverlust ist behauptet.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Erhaltene lesbare lokale Akten dienen noch der Abwicklung. Die entfallene Außenfunktion ist nicht erhalten.

**Zu prüfen:** Ein legitim nutzbarer kompatibler Teilnehmer führt genau diese Funktion im benötigten Zeitraum aus.

**Architekturfolge für diesen Stressor:** Lokale Baufähigkeit und Providerberechtigung getrennt inventarisieren. Laufendes Binary ist kein Beweis zukünftiger Reparierbarkeit.

<a id="s293"></a>
## S293 — Nach tausend Sonderfällen versteht niemand mehr welche Ausnahme Außenwirkung erlaubt

**Ursprung:** boundaries; A6-Zustände: BD14, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s293.b01"></a>
### S293.B01 — Ausnahmen reproduzieren neue Ausnahmen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Dringliche Entscheidungen werden durch Bypass gelöst und jeder Bypass wird neuer unklarer Präzedenzfall. Unabhängig erhaltene Entscheidungen sind noch lesbar.

**Warum bleibt oder endet der Zustand?** Unklare Ausnahme → Bypass → zusätzlicher Präzedenzfall → mehr Unklarheit → nächster Bypass.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann dokumentierte Entscheidungsfragmente untersuchen aber keine einheitliche Legitimität daraus erfinden.

**Zu prüfen:** Gegenfälle werden dauerhaft nach gleicher verstandener Regel entschieden ohne neue Ausnahmen.

<a id="s293.b02"></a>
### S293.B02 — Unklare Autorität hält neue Aktion an

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Wirksame lokale Grenze verweigert unaufgelöste Ausnahme und zuständiger Entscheider kann widersprechende Weisungen prüfen.

**Warum bleibt oder endet der Zustand?** Halt bleibt bis kompetente Vorrangentscheidung oder Abbruch. Gepufferte Teilnehmerarbeit ist gesondert offen.

**Zugeordnete Residues:** [BDR015: Erklärbare Autoritätsentscheidung](residues.md#bdr015)

**Was bleibt warum nutzbar?** Entscheider behält Konflikt und Begründungsbedarf als nutzbaren Gegenstand statt neuer stiller Ausnahme.

**Zu prüfen:** Neue Außenaktion passiert trotz offenem Konflikt oder Regel wird ohne zuständige Autorität geraten.

**Architekturfolge für diesen Stressor:** Wirksame Ausnahmeentscheidungen mit expliziter Vorrangregel dokumentieren. Regelzahl allein ist keine Dynamik und ein Entscheidungsregister ist keine universelle Außenaktionssperre.

<a id="s294"></a>
## S294 — Ein Update der gemeinsamen JSON-Bibliothek verändert gleichzeitig Plugins Replay und Monitoring

**Ursprung:** boundaries; A6-Zustände: BD15, BD04, BD05, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s294.b01"></a>
### S294.B01 — Gemeinsamer Parserausfall legt betroffene Pfade still

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Neue Bibliothek verwirft erforderliche Daten in Plugins Replay und Monitoring. Unabhängiger lokaler Reader des archivierten Formats bleibt erreichbar.

**Warum bleibt oder endet der Zustand?** Pflichtabhängigkeit hält Pfade aus bis kompatibler Parser verfügbar wird. Kein autonomer Fehlerkreis.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Offlinebetreiber kann den erhaltenen älteren Bestand lesen sofern sein Reader nicht die neue Bibliothek braucht.

**Zu prüfen:** Auch der angeblich unabhängige Reader hängt von der kaputten Bibliothek ab.

<a id="s294.b02"></a>
### S294.B02 — Einmalige plausible gemeinsame Fehldeutung

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Alle aktualisierten Verbraucher lesen falsche Menge einmal. Unabhängiger Mengenursprung wurde nicht mit aktualisiert.

**Warum bleibt oder endet der Zustand?** Der einzelne falsche Lauf endet als Fehlresultat. Wiederholte Vertrauensverstärkung ist noch nicht gegeben.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Fachprüfer kann die falsche Menge gegen den separaten Ursprung erkennen. Mehr gleiche JSON-Prüfer helfen nicht.

**Zu prüfen:** Unabhängiger Sollvergleich zeigt dieselbe richtige Menge oder Ursprung nutzt dieselbe fehlerhafte Interpretation.

<a id="s294.b03"></a>
### S294.B03 — Parserfehler wird kanonisch bestätigt

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Replay und Monitor akzeptieren dieselbe falsche Deutung und diese wird Ursprung weiterer Entscheidungen. Ein getrennter Mengenbezug bleibt für befugten Eingriff erreichbar aber wird bisher ignoriert.

**Warum bleibt oder endet der Zustand?** Fehldeutung → zustimmende Checks → Vertrauen → Übernahme als Ursprung → wieder zustimmende Checks.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Unabhängiger Fachprüfer könnte mit dem noch erhaltenen Ursprung widersprechen. Erst wirksamer Eingriff bricht Vertrauen und wird nicht vorausgesetzt.

**Zu prüfen:** Gegenbezug beeinflusst tatsächlich die Adoption oder keine weitere Datenübernahme reproduziert den Fehler.

**Architekturfolge für diesen Stressor:** Gemeinsame JSON-Abhängigkeit nicht als unabhängigen Prüfer behandeln. Semantischen Gegenvergleich und unveränderte Ursprungsbytes vor Upgrade sichern.

<a id="s295"></a>
## S295 — Ein Monitoringfeature macht einen Cloudaccount zur Startvoraussetzung aller Kindscopes

**Ursprung:** boundaries; A6-Zustände: BD15, BD37. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s295.b01"></a>
### S295.B01 — Pflichtcloud verhindert alle Kindstarts

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Monitoringauthentifizierung ist wirklich globaler Startprädikat und Konto fehlt. Lokale Akten und unabhängiger Notreader existieren weiterhin.

**Warum bleibt oder endet der Zustand?** Starts warten auf Konto oder Entkopplung. Bestehende Daten werden nicht allein durch fehlenden Login gelöscht.

**Zugeordnete Residues:** [BDR011: Unabhängig lesbarer Notstart](residues.md#bdr011), [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Berechtigter lokaler Betreiber kann Konfiguration und erhaltenen Bestand lesen. Regulärer Kindbetrieb ist nicht erhalten.

**Zu prüfen:** Auch Notreader oder lokale Akten sind nur nach demselben Cloudlogin zugänglich.

<a id="s295.b02"></a>
### S295.B02 — Nicht aktivierender Kindscope bleibt lokal nutzbar

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Optionales Monitoring ist aus dem Startvertrag des Kindes entfernt und dessen benötigte Ressourcen sind unabhängig.

**Warum bleibt oder endet der Zustand?** Der Cloudfehler betrifft nur aktivierende Pfade solange diese Isolationsannahme gilt.

**Zugeordnete Residues:** [BDR012: Scopebegrenzte Startfähigkeit](residues.md#bdr012)

**Was bleibt warum nutzbar?** Kindscopenutzer behalten lokale Abfragen und Start ohne das zusätzliche Konto. Das ist ein alternativer begrenzter Entwurf.

**Zu prüfen:** Ein anderer globaler Startschritt erzwingt weiterhin dasselbe Monitoringkonto.

**Architekturfolge für diesen Stressor:** Monitoringaktivierung scopeweise halten und lokalen Inspektionsstart ohne Cloudkonto ermöglichen. Cloudfreiheit dieses Pfads ist keine Dienstgarantie aller Plugins.

<a id="s296"></a>
## S296 — Eine hilfreiche automatische Reparatur wächst zum zweiten schreibenden Kernel

**Ursprung:** boundaries; A6-Zustände: BD08, BD04, BD27. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s296.b01"></a>
### S296.B01 — Reparaturhelper schreibt nicht unabhängig

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Helper hat keine direkte Mutationserlaubnis und alle Änderungen passieren denselben durchgesetzten Writervertrag.

**Warum bleibt oder endet der Zustand?** Ein Reparaturvorschlag wird serialisiert oder abgewiesen. Keine Gegenreparaturschleife aus zwei unabhängigen Autoritäten.

**Zugeordnete Residues:** [BDR008: Teilnehmerseitige Schreibgeneration](residues.md#bdr008)

**Was bleibt warum nutzbar?** Der berechtigte Kernel behält exklusive Mutation an den erfassten Teilnehmern. Andere Schreibwege müssen ausdrücklich ausgeschlossen sein.

**Zu prüfen:** Helper verändert Store oder Außenwelt unter eigener unbegrenzter Writerautorität.

<a id="s296.b02"></a>
### S296.B02 — Zwei Kernel korrigieren einander dauerhaft

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Helper und Kernel haben wirksame Schreibrechte und definieren Änderungen des jeweils anderen als zu reparierende Drift. Getrennte Belegkopie überlebt.

**Warum bleibt oder endet der Zustand?** Kerneländerung → Helperkorrektur → Kernelkorrektur → neue Helperkorrektur nach Ende des ersten Incidents.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer behält erhaltene Konfliktspur und kann die doppelte Autorität untersuchen. Kein eigener Schreibstopp folgt aus dem Beleg.

**Zu prüfen:** Nach einmaliger Korrektur bleibt Zustand ohne weitere Gegenreaktion stabil oder ein Writer ist effektiv ausgeschlossen.

<a id="s296.b03"></a>
### S296.B03 — Einmalige unerlaubte Korrektur

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Helper mutiert einmal falsch aber kein erneutes Reparieren folgt. Unabhängige Vorher-Nachher-Belege sind erhalten.

**Warum bleibt oder endet der Zustand?** Ein falscher Abschluss ist keine Schleife. Prüfung und Korrektur sind weitere Arbeit.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann den belegten Eingriff abgrenzen. Die Spur belegt nicht automatisch alle externen Folgen.

**Zu prüfen:** Kein falscher Write trat auf oder wiederholte Gegenkorrekturen zeigen doch einen Rückkopplungszweig.

**Architekturfolge für diesen Stressor:** Reparaturhelper dürfen nur Kommandos an dieselbe Schreibautorität liefern. Schreibkonfliktbeleg allein verhindert keinen unabhängigen zweiten Kernel.

<a id="s297"></a>
## S297 — Alle Tests verwenden sofortige Antworten und verpassen jede verspätete Annahme

**Ursprung:** boundaries; A6-Zustände: BD16, BD03, BD10, BD24. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s297.b01"></a>
### S297.B01 — Soforttests lassen Produktionsausgang offen

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Testbestand enthält ausschließlich sofortige Antworten. Keine unabhängige spätere Annahmespur ist bisher vorhanden.

**Warum bleibt oder endet der Zustand?** Tests können beliebig oft bestehen ohne die ausgelassene Grenzfolge zu entscheiden.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für reale Sicherheit bei verspäteter Annahme fehlt erforderliche Evidenz. Soforttests bleiben nur für ihre tatsächlich abgebildeten Abläufe nutzbar.

**Zu prüfen:** Eine getrennt gesteuerte spät angenommene Operation unterscheidet sicheren und fehlerhaften Vertrag.

<a id="s297.b02"></a>
### S297.B02 — Bekannter Versuch wartet trotz Timeout

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Produktion oder isolierter Entwurf speichert Intent vor Versand und hält nach Timeout weitere lokale Versuche zurück.

**Warum bleibt oder endet der Zustand?** Unbekannter Ausgang bleibt bis Teilnehmerabgleich offen. Alte Annahme kann dennoch eintreffen.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Operator kann die bekannte Operation wiederfinden ohne Antwortverlust als Ablehnung umzudeuten. Es ist keine Provider-Deduplikation.

**Zu prüfen:** Ein Timeout führt ohne Aufklärung automatisch zu neuem wirksamem Versuch.

<a id="s297.b03"></a>
### S297.B03 — Separater Verzögerungsprüflauf deckt Lücke auf

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Neu entworfener isolierter Prüflauf steuert Annahme nach Timeout unabhängig und dokumentiert Teilnehmerannahmen ausdrücklich.

**Warum bleibt oder endet der Zustand?** Ein endlicher Nachweis über diese Spur ist abgeschlossen. Andere Produktionsverträge bleiben unbewiesen.

**Zugeordnete Residues:** [BDR016: Abgegrenzter Prüflaufbeleg](residues.md#bdr016)

**Was bleibt warum nutzbar?** Testprüfer kann die ausgelassene Interleavingklasse beurteilen und Mockgrenzen sehen. Kein empirischer Providerbeweis wird behauptet.

**Zu prüfen:** Defekt bei später Annahme bleibt im getrennten Lauf genauso unsichtbar wie in Sofortfixtures.

**Architekturfolge für diesen Stressor:** Testbeleg und reale Annahme separat führen. Versuchsregister schützt nur bekannte lokale Wiederholung und benötigt negative Spuren mit später Annahme.

<a id="s298"></a>
## S298 — Die virtuelle Testuhr friert Timeout und externen Monitor gemeinsam ein

**Ursprung:** boundaries; A6-Zustände: BD16. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s298.b01"></a>
### S298.B01 — Gemeinsam eingefrorener Test beweist keine Erkennung

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Timeout und Testmonitor teilen die eingefrorene virtuelle Uhr und keine unabhängig fortschreitende Beobachtung liegt vor.

**Warum bleibt oder endet der Zustand?** Beide schweigen solange die gemeinsame Testzeit steht. Keine Aussage über reale Monitoruhr folgt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Erkennung einer Produktionszeitstörung fehlt unterscheidende Evidenz. Es wird weder reale Blindheit noch realer Notalarm behauptet.

**Zu prüfen:** Ein unabhängiger Prüfer läuft während eingefrorener Taskuhr weiter und zeigt die fehlende Fristreaktion.

<a id="s298.b02"></a>
### S298.B02 — Getrennte Testuhr zeigt den fehlenden Timeout

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein tatsächlich unabhängig gesteuerter Prüfer registriert denselben eingefrorenen Taskablauf und hält seine Annahmen fest.

**Warum bleibt oder endet der Zustand?** Der konkrete negative Test endet mit nachgewiesener Diskrepanz. Produktwirksamkeit benötigt einen anderen Nachweis.

**Zugeordnete Residues:** [BDR016: Abgegrenzter Prüflaufbeleg](residues.md#bdr016)

**Was bleibt warum nutzbar?** Testverantwortlicher kann das gemeinsame Einfrieren widerlegen statt es mit einem zweiten gleich getakteten Mock zu bestätigen.

**Zu prüfen:** Auch der neue Prüfer friert implizit mit oder kann keine Abweichung erkennen.

**Architekturfolge für diesen Stressor:** Prüferuhr vom Tasktimer trennen und Testzeit nicht als Betriebsevidenz ausgeben. Ohne unabhängige Uhr bleiben reale Dauer und Erkennung offen.

<a id="s299"></a>
## S299 — Ein Mock bestätigt Fencing obwohl der reale Provider alte Generationen akzeptiert

**Ursprung:** boundaries; A6-Zustände: BD16, BD08, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s299.b01"></a>
### S299.B01 — Mockbeleg hat keine Aussage über reale Exklusion

**Art:** offen. **Residue-Status:** teilweise.

**Voraussetzungen:** Mock meldet Erfolg aber realer Provider nimmt alte Generationen an. Spuren dokumentieren nur Mockannahmen.

**Warum bleibt oder endet der Zustand?** Bestandene Mocktests ändern die reale Annahmeregel nicht.

**Zugeordnete Residues:** [BDR016: Abgegrenzter Prüflaufbeleg](residues.md#bdr016)

**Was bleibt warum nutzbar?** Testprüfer kann den begrenzten Fixturevertrag inspizieren und seine Widerlegung dokumentieren. Eine reale Sperrfähigkeit bleibt nicht erhalten.

**Zu prüfen:** Nachvollziehbare tatsächliche Teilnehmerannahmespur weist entgegen der Prämisse alle alten Generationen zurück.

<a id="s299.b02"></a>
### S299.B02 — Falsches Fencing erhält gegeneinander arbeitende Writer

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Beide Writer bleiben am Provider wirksam und reparieren wiederholt gegeneinander. Getrennte Konfliktevidenz ist zugänglich.

**Warum bleibt oder endet der Zustand?** Alte Änderung → neue Reparatur → alte Reparatur → erneute Änderung trotz lokaler Fencingbestätigung.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann dokumentierte Doppelautorität nachweisen. Das lokale Erfolgssignal ist gerade kein Ausschlussresidue.

**Zu prüfen:** Keine Gegenreparatur folgt oder tatsächliche Annahmeprüfung schließt einen Writer aus.

<a id="s299.b03"></a>
### S299.B03 — Wirksamer Teilnehmervertrag ersetzt Mockannahme

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alternativer Entwurf erzwingt alte Generationen einschließlich Puffer am realen letzten Effektschritt abzulehnen.

**Warum bleibt oder endet der Zustand?** Altwriter bleibt ausgeschlossen solange die dauerhafte Teilnehmerregel gilt.

**Zugeordnete Residues:** [BDR008: Teilnehmerseitige Schreibgeneration](residues.md#bdr008)

**Was bleibt warum nutzbar?** Neuer legitimer Writer behält exklusive Annahmefähigkeit. Das setzt einen gegenüber dem Szenario geänderten Vertrag voraus und ist kein heutiger Befund.

**Zu prüfen:** Ein vor Fencing gepufferter Altauftrag wird nach bestätigtem Ausschluss angenommen.

**Architekturfolge für diesen Stressor:** Fencingrückmeldung an tatsächliche Annahme alter Generationen binden. Testspur und echter Ausschluss sind verschiedene Residues.

<a id="s300"></a>
## S300 — Ein erfolgreicher Bericht vertauscht Euro Cent und Dezimalkomma

**Ursprung:** boundaries; A6-Zustände: BD04, BD05, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s300.b01"></a>
### S300.B01 — Bericht deutet Menge falsch Zahlung noch ungeklärt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Bericht vertauscht Einheiten oder Locale. Ein getrennter autorisierter Ursprungsbetrag ist vorhanden aber tatsächlicher Zahlungsbeleg fehlt.

**Warum bleibt oder endet der Zustand?** Anzeigefehler bleibt bis Korrektur. Wirkungsausgang bleibt separat offen.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Fachprüfer kann den Darstellungsfehler gegen Originalmenge erkennen ohne daraus eine falsche Außenwirkung zu erfinden.

**Zu prüfen:** Unabhängiger Normalvergleich zeigt korrekte Darstellung oder ein tatsächlicher Zahlungsbeleg klärt den Ausgang.

<a id="s300.b02"></a>
### S300.B02 — Einheitvertrag hält falschen Bericht vor Nutzung zurück

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Wert Einheit Skalierung und Locale werden vor Abnahme unabhängig gegen vereinbarte Semantik geprüft.

**Warum bleibt oder endet der Zustand?** Bericht wird bis korrigierter Darstellung nicht als Erfolgsevidenz genutzt.

**Zugeordnete Residues:** [BDR005: Dimensionsgebundener Wertevertrag](residues.md#bdr005)

**Was bleibt warum nutzbar?** Abnehmer kann die beabsichtigte Menge verständlich lesen oder diese Darstellung ablehnen. Gemeinsame falsche Sollquelle bleibt Grenze.

**Zu prüfen:** Euro und Cent werden in der synthetischen Kreuzversion dennoch gleich akzeptiert.

<a id="s300.b03"></a>
### S300.B03 — Formatter bestätigt seine eigenen falschen Berichte

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Gleicher Formatter wird Prüfer und seine Zustimmung stärkt weitere ungeprüfte Berichtsnutzung. Ein separater Mengenursprung bleibt zugänglich aber ungenutzt.

**Warum bleibt oder endet der Zustand?** Falscher Bericht → gleich falsch zustimmende Prüfung → mehr Vertrauen → weitere Wiederverwendung → falscher Bericht.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Ein befugter unabhängiger Abnehmer könnte mit dem erhaltenen Ursprung widersprechen. Sein Eingriff ist nicht automatisch gegeben.

**Zu prüfen:** Keine Vertrauensverstärkung oder ein wirklich unabhängiger Abgleich bestimmt fortan die Abnahme.

**Architekturfolge für diesen Stressor:** Berichtsformat und tatsächliche Menge gesondert abnehmen. Falsche Anzeige allein beweist keine falsche Zahlung und gleiche Formatter sind kein unabhängiger Check.

<a id="s301"></a>
## S301 — Ein schneller Cache liefert eine alte aber plausibel richtige Preisliste für heutige Bestellung

**Ursprung:** boundaries; A6-Zustände: BD04, BD01, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s301.b01"></a>
### S301.B01 — Alter fixierter Preis ist für diese Bestellung richtig

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein erhaltenes gültiges Angebot bindet genau diese Bestellung legitim an den alten Preis und unabhängiger Abgleich bestätigt den richtigen Abschluss.

**Warum bleibt oder endet der Zustand?** Der begrenzte Kauf kann korrekt abgeschlossen sein obwohl Cache alt ist.

**Zugeordnete Residues:** [BDR006: Gebundene Preiszusage](residues.md#bdr006)

**Was bleibt warum nutzbar?** Berechtigter Abnehmer kann die gültige Preisbindung prüfen und verwenden. Es folgt keine Zusage für andere heutige Bestellungen.

**Zu prüfen:** Zusage ist abgelaufen nicht bindend oder betrifft ein anderes Produkt beziehungsweise Bestellung.

<a id="s301.b02"></a>
### S301.B02 — Abgelaufene erhaltene Preiszusage darf heutige Bestellung nicht binden

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein echter alter Preisbeleg mit eindeutiger Gültigkeitsgrenze ist erhalten. Heutige Bindung ist erforderlich und wirksame Annahmeprüfung erkennt dass diese Zusage abgelaufen ist.

**Warum bleibt oder endet der Zustand?** Bestellung wartet auf aktuelle legitime Zusage statt Plausibilität als Frische zu deuten.

**Zugeordnete Residues:** [BDR006: Gebundene Preiszusage](residues.md#bdr006)

**Was bleibt warum nutzbar?** Der Preisvertragsprüfer kann den erhaltenen Beleg zur Feststellung der abgelaufenen Bindung nutzen und betroffene Annahme verweigern. Ohne jeden echten Preisbeleg wäre nur der Cachewert kein solcher Gegenstand.

**Zu prüfen:** Abgelaufener alter Preis wird trotz belegter Gültigkeitsgrenze ohne aktuelle Zusage angenommen.

<a id="s301.b03"></a>
### S301.B03 — Veralteter Preis bereits verwendet

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Bestellung verwendete ungebundenen alten Preis entgegen tatsächlicher Preisregel. Alte und aktuelle unabhängig zugängliche Belege sind erhalten.

**Warum bleibt oder endet der Zustand?** Falscher Abschluss bleibt bis möglicher neuer Korrekturarbeit. Kein Rückkopplungskreis ist gegeben.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Fachprüfer kann die belegte Regelabweichung untersuchen. Historische Preisbelege ersetzen keine neue gültige Bestellung.

**Zu prüfen:** Die maßgebliche Regel erlaubte genau den alten Preis oder tatsächliche Annahme nutzte aktuellen Preis.

**Architekturfolge für diesen Stressor:** Preisalter von rechtlicher Angebotsbindung trennen. Cachegültigkeit muss für diese Bestellung entschieden werden nicht über allgemeine Frischeetiketten.

<a id="s302"></a>
## S302 — Ein Auftrag wird vollständig für die falsche reale Person mit gleichem Namen erledigt

**Ursprung:** boundaries; A6-Zustände: BD04, BD06, BD10, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s302.b01"></a>
### S302.B01 — Namensgleiche Personen vor Wirkung getrennt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängiger legitimer Realempfängerbezug widerspricht dem ausgewählten Ziel und kontrollierte Lieferung wartet.

**Warum bleibt oder endet der Zustand?** Falsches Ziel bleibt gesperrt bis Identität und Zielkanal kompetent geklärt sind.

**Zugeordnete Residues:** [BDR007: Bestätigter Realempfängerbezug](residues.md#bdr007)

**Was bleibt warum nutzbar?** Bearbeiter kann den richtigen Menschen vom gleichnamigen anderen unterscheiden. Ein falsch formulierter Originalauftrag bleibt ungesichert.

**Zu prüfen:** Identifikationsprüfung akzeptiert vertauschte synthetische Personen oder falschen Zielkanal.

<a id="s302.b02"></a>
### S302.B02 — Arbeit vollständig für falsche Person erledigt

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Tatsächliche Lieferung betrifft andere Person. Beabsichtigter unabhängiger Realempfängerbeleg bleibt zugänglich.

**Warum bleibt oder endet der Zustand?** Ein technischer Erfolg ist hier falscher Fachabschluss. Spätere Korrektur ist neue Arbeit.

**Zugeordnete Residues:** [BDR007: Bestätigter Realempfängerbezug](residues.md#bdr007)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann die Fehlzuordnung feststellen. Vergangene Zustellung wird dadurch nicht aufgehoben.

**Zu prüfen:** Unabhängiger Personen- und Wirkungsbeleg zeigt dass die beabsichtigte reale Person bedient wurde.

<a id="s302.b03"></a>
### S302.B03 — Falsche Person hat geschützte Inhalte gelernt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Fehllieferung enthielt vertrauliche Daten und unbefugte andere Person hat sie gelesen. Ein erlaubter Transferbeleg bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Historische Vertraulichkeit ist verloren auch nach Rückruf. Weiterer Zugriff ist gesondert begrenzbar.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann den belegten Offenlegungsumfang untersuchen ohne eine zusätzliche geheime Vollkopie zu benötigen. Vergessen ist nicht nachweisbar.

**Zu prüfen:** Inhalt war nicht geschützt oder anderer Empfänger konnte ihn nachweislich nicht lesen.

**Architekturfolge für diesen Stressor:** Realen Empfänger von internen IDs und Anzeigenamen trennen. Empfängerbeleg erhält Zuordnung nicht rückwirkend Geheimhaltung nach Fehllieferung.

<a id="s303"></a>
## S303 — Im Incident ist die rote Warnung für einen farbfehlsichtigen Operator nicht erkennbar

**Ursprung:** boundaries; A6-Zustände: BD17, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s303.b01"></a>
### S303.B01 — Warnung bleibt für diesen Operator unverständlich

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Rot ist einzige Bedeutungsträgerin und kein anderer effektiver Empfänger handelt. Technische Alarmbelege bleiben zugänglich.

**Warum bleibt oder endet der Zustand?** Informationslücke dauert bis nutzbarer Darstellung oder anderem legitimen Eingriff. Kein autonomer Kreis.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Späterer Prüfer kann die vorhandene Warnung nachvollziehen. Rechtzeitiges Verstehen dieses Operators ist nicht erhalten.

**Zu prüfen:** Dieser Operator erkennt in der tatsächlichen Ansicht unabhängig von Farbe Zustand und Ziel rechtzeitig korrekt.

<a id="s303.b02"></a>
### S303.B02 — Text und Symbol erhalten Bedeutung

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Geprüfter alternativer Alarmvertrag bietet diesem Operator verständlichen Text und redundante Markierung auf seinem Gerät.

**Warum bleibt oder endet der Zustand?** Darstellungsstörung endet für diese Nachricht wenn Bedeutung erkannt wird. Reparatur bleibt eigene Handlung.

**Zugeordnete Residues:** [BDR017: Eindeutig interpretierbare Alarmansicht](residues.md#bdr017)

**Was bleibt warum nutzbar?** Erreichter berechtigter Operator kann Alarm und erlaubten nächsten Schritt korrekt unterscheiden. Kein Notfallstopp wird damit bewiesen.

**Zu prüfen:** Synthetischer Test zeigt trotz Text und Symbol weiterhin falsche Zustands- oder Zielzuordnung.

**Architekturfolge für diesen Stressor:** Alarme mehrkanalig in ihrer Bedeutung darstellen und operatorgerecht prüfen. Sichtbarer Alarm ist weder Verständnis noch rechtzeitige physische Intervention.

<a id="s304"></a>
## S304 — Ein langer Scopepfad wird im Mobilalarm gekürzt und der Operator stoppt die falsche Instanz

**Ursprung:** boundaries; A6-Zustände: BD04, BD17, BD10, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s304.b01"></a>
### S304.B01 — Falsche Instanz gestoppt richtige bleibt gestört

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Gekürzte Pfade kollidieren und Operator stoppte tatsächlich das andere Ziel. Zeitlich und sachlich getrennte Belege bleiben vorhanden.

**Warum bleibt oder endet der Zustand?** Falscher Eingriff ist abgeschlossen während ursprünglicher Incident weiterbestehen kann.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann belegte Zielverwechslung untersuchen. Weder richtige Instanzreparatur noch Unschädlichkeit des Stopps folgt daraus.

**Zu prüfen:** Stoppbeleg zeigt das tatsächlich beabsichtigte eindeutige Ziel oder keine Aktion wurde angenommen.

<a id="s304.b02"></a>
### S304.B02 — Vollidentitätsprüfung hält mehrdeutigen Stopp

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Eindeutige ungekürzte Identität ist unabhängig vom Anzeigenpfad verfügbar und muss vor Stopp bestätigt werden.

**Warum bleibt oder endet der Zustand?** Stopp wartet auf gültige eindeutige Zuordnung. Lesbarkeit alleine schließt Irrtum nicht vollkommen aus.

**Zugeordnete Residues:** [BDR017: Eindeutig interpretierbare Alarmansicht](residues.md#bdr017)

**Was bleibt warum nutzbar?** Operator kann Ziel und konkrete Aktion auf kleinem Gerät prüfen. Die bestätigte Ansicht muss an denselben angenommenen Zielbezug gebunden sein.

**Zu prüfen:** Zwei kollidierende Kurzpfade führen trotz Bestätigung zum selben ununterscheidbaren Stopptarget.

**Architekturfolge für diesen Stressor:** Vollständige stabile Zielidentität vor Stoppausführung anzeigen und bestätigen. Mobilkürzung darf nicht zum Eingriffsschlüssel werden.

<a id="s305"></a>
## S305 — Eine übersetzte Schaltfläche macht aus unklar scheinbar sicher fehlgeschlagen

**Ursprung:** boundaries; A6-Zustände: BD04, BD10, BD03, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s305.b01"></a>
### S305.B01 — Übersetzung erzeugt falschen Ausgangsglauben

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Anzeige sagt fehlgeschlagen obwohl Ausgang unklar ist. Originale unklare Versuchsevidenz ist separat erhalten und noch kein neuer Effekt belegt.

**Warum bleibt oder endet der Zustand?** Falscher Glaube bleibt bis Klärung. Eine daraus folgende Doppelwirkung ist möglich aber nicht vorausgesetzt.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann die verfälschte Deutung gegen originale Aussage prüfen. Der falsche UI-Glaube beweist keine erfolgte Doppelzahlung.

**Zu prüfen:** Operator versteht trotz Übersetzung korrekt unklar oder tatsächliche Ablehnung ist unabhängig belegt.

<a id="s305.b02"></a>
### S305.B02 — Unklar bleibt sichtbar und Wiederholung gehalten

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Anzeige erhält Originalstatus und ein erhaltenes Versuchsregister bindet lokale Wiederholung tatsächlich an Klärung.

**Warum bleibt oder endet der Zustand?** Outcome bleibt offen bis korreliertem Teilnehmerbeleg. Verständliche Anzeige allein löst ihn nicht.

**Zugeordnete Residues:** [BDR017: Eindeutig interpretierbare Alarmansicht](residues.md#bdr017), [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Operator kann Ungewissheit verstehen und bekannten Versuch ohne blindes Retry abgleichen. Empfang und Autorität sind separate Voraussetzungen.

**Zu prüfen:** Operator liest eindeutig fehlgeschlagen oder UI erlaubt automatische Wiederholung ohne offene Versuchsevidenz zu beachten.

**Architekturfolge für diesen Stressor:** Unklar und abgelehnt als unvertauschbare Zustände durch Übersetzung und Aktionsangebot führen. Falscher Glaube ist noch kein falsches Werkprodukt.

<a id="s306"></a>
## S306 — Der Alarm erreicht nachts ein ausgeschaltetes Telefon und niemand bemerkt ihn für Tage

**Ursprung:** boundaries; A6-Zustände: BD17. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s306.b01"></a>
### S306.B01 — Nachricht vorhanden aber kein Mensch erreicht

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Einzig vorgesehenes Telefon ist aus und im relevanten Zeitraum ist kein legitimer anderer Empfänger erreichbar. Ein lesbarer Sendebeleg überlebt.

**Warum bleibt oder endet der Zustand?** Warten endet erst mit späterem Empfang oder Aufgabe. Mehrere Tage sind kein eigener Rückkopplungsmechanismus.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Späterer Prüfer kann den Sendeversuch nachvollziehen. Für rechtzeitige menschliche Reaktion gibt es in diesem Zweig keine Restfähigkeit.

**Zu prüfen:** Ein legitimer Mensch bestätigt tatsächlich rechtzeitiges Verständnis und Handeln über anderen Weg.

<a id="s306.b02"></a>
### S306.B02 — Endliche Nachricht findet unabhängigen Empfänger

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Alternativer Entwurf hat noch laufende Empfangsfrist und einen tatsächlich unabhängigen erreichbaren berechtigten Empfängerweg.

**Warum bleibt oder endet der Zustand?** Ausbleibende Quittung löst gezielte Umleitung dieser Nachricht aus. Deren Empfang endet nicht den Incident.

**Zugeordnete Residues:** [BDR018: Begrenzter Zustell- und Empfangsnachweis](residues.md#bdr018)

**Was bleibt warum nutzbar?** Disponent kann fehlenden menschlichen Empfang erkennen und eine begrenzte Nachricht rechtzeitig weiterreichen. Kein unbegrenzter Bereitschaftsdienst folgt.

**Zu prüfen:** Ausweichweg hängt am selben ausgeschalteten Gerät oder nur Transportannahme wird als menschlicher Empfang gezählt.

**Architekturfolge für diesen Stressor:** Menschlichen Empfang und Ende der Zustellpflicht separat vom Providerack führen. Keine Ausweichbereitschaft erfinden wo nachts niemand erreichbar ist.

<a id="s307"></a>
## S307 — Ein Flapping-Link erzeugt zehntausend Meldungen und verdeckt den späteren echten Totalausfall

**Ursprung:** boundaries; A6-Zustände: BD17, BD18. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s307.b01"></a>
### S307.B01 — Endlicher Meldungsberg wird abgearbeitet

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Flapping endet und Bearbeitung übersteigt neue Meldungen. Separates kapazitiv ausreichendes Vorfallsregister führt den echten Totalausfall unabhängig von Wiederholungen.

**Warum bleibt oder endet der Zustand?** Rückstand sinkt bei unveränderter Grundlast bis abgearbeitet. Kein Attraktor aus der Zahl zehntausend allein.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Einsatzplanung kann den gesonderten echten Ausfall weiterfinden. Das beweist noch keine Behebung oder rechtzeitige menschliche Aufmerksamkeit.

**Zu prüfen:** Neuer Totalausfall verschwindet bei Zusammenfassung oder Rückstand wächst nach Ende der Flaps aus inneren Erinnerungen weiter.

<a id="s307.b02"></a>
### S307.B02 — Erinnerungen verdrängen Reparatur dauerhaft

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Ungelöste Fehler erzeugen nach Ende der Flaps mehr Aufmerksamkeitspflichten als abgearbeitet werden. Vorfallsregister selbst überlebt mit benannter Kapazität.

**Warum bleibt oder endet der Zustand?** Mehr Rückstand → weniger Reparaturzeit → mehr ungelöste Vorfälle → mehr Erinnerungen → mehr Rückstand.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Berechtigte Einsatzplanung behält registrierte offene Vorfälle sichtbar. Erhaltene Liste ersetzt keine freie Bearbeitungskapazität.

**Zu prüfen:** Bei gleicher Grundlast und Personalstärke sinkt Rückstand nach Störungsende dauerhaft ohne zusätzliche innere Produktion.

**Architekturfolge für diesen Stressor:** Meldungsdeduplikation vom offenen Vorfallsregister trennen. Echten Totalausfall trotz endlichem Burst erhalten und Schleifen nur nach Ende der Flaps behaupten.

<a id="s308"></a>
## S308 — Der Operator bestätigt einen Alarm sofort aber die Reparatur liegt drei Monate in seiner Queue

**Ursprung:** boundaries; A6-Zustände: BD19, BD18, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s308.b01"></a>
### S308.B01 — Quittiert aber drei Monate zurückgestellt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Planung erlaubt langen Aufschub und Alarmack wird nicht als Reparatur ausgegeben. Offenes Vorfallsregister bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Bewusste Queuepolitik hält Aufschub bis tatsächlicher Bearbeitung oder ausdrücklicher Aufgabe.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Berechtigte Planung kann Fristverletzung und offenen Reparaturbedarf sehen obwohl Pagerpflicht beendet ist.

**Zu prüfen:** Quittung löscht den Vorfall oder tatsächliche Reparatur ist bereits unabhängig verifiziert.

<a id="s308.b02"></a>
### S308.B02 — Rückstand erzeugt neue Reparaturpflichten

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Aufschub lässt Fehler weiterleben und ihre Meldungen verbrauchen die verbleibende Reparaturzeit. Register bleibt lesbar.

**Warum bleibt oder endet der Zustand?** Aufschub → ungelöste Fehler → Meldungsarbeit → weniger Reparatur → mehr Aufschub bei fester Grundlast.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Planer kann noch gespeicherte offene Vorfälle priorisieren. Bestehende Kapazitätskrise wird nicht vom Register gelöst.

**Zu prüfen:** Nach Anfangsrückstand werden Fehler ohne innere Neubelastung kontinuierlich behoben.

<a id="s308.b03"></a>
### S308.B03 — Konkreter Vorfall fachlich behoben

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Reparatur wird tatsächlich durchgeführt und unabhängiger aufgabenspezifischer Check bestätigt Wirkung. Vorfalls- und Abnahmebeleg bleiben zugänglich.

**Warum bleibt oder endet der Zustand?** Nur dieser Vorfall ist abgeschlossen. Andere quittierte Incidents bleiben offen.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019), [BDR032: Aufgabenspezifische Artefaktabnahme](residues.md#bdr032)

**Was bleibt warum nutzbar?** Planer und Abnehmer können echte Behebung vom alten Ack unterscheiden und den begrenzten Abschluss nachvollziehen.

**Zu prüfen:** Abschlussprüfung misst nur Ack oder betroffene Funktion ist weiterhin defekt.

**Architekturfolge für diesen Stressor:** Quittung Zuständigkeit Aufschub und geprüfte Behebung als getrennte Vorfallszustände führen. Reparaturfrist nicht mit Alarmack erfüllen.

<a id="s309"></a>
## S309 — Zwei rechtmäßige Vertreter erteilen gleichzeitig gegensätzliche Freigaben

**Ursprung:** boundaries; A6-Zustände: BD20, BD01, BD09, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s309.b01"></a>
### S309.B01 — Gegensätzliche Freigaben halten Aktion an

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Vertreter sind legitim aber keine kompetente Vorrangregel entscheidet. Lokale Grenze hält neue Aktion tatsächlich zurück.

**Warum bleibt oder endet der Zustand?** Konflikt bleibt bis zuständiger Entscheidung oder Abbruch. Keine autonome Konvergenz.

**Zugeordnete Residues:** [BDR015: Erklärbare Autoritätsentscheidung](residues.md#bdr015)

**Was bleibt warum nutzbar?** Entscheider kann beide Weisungen mit Geltung prüfen ohne sie durch last-write-wins zu verfälschen. Alte Puffer bleiben separat zu prüfen.

**Zu prüfen:** Ein Effekt passiert trotz offenem Konflikt oder bindende vorhandene Regel hätte bereits eindeutig entschieden.

<a id="s309.b02"></a>
### S309.B02 — Kompetente Ordnung wird bei Annahme angewandt

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorhandene gültige Vorrangregel löst genau diesen Konflikt und jeder betreffende Teilnehmer prüft deren aktuelle Geltung.

**Warum bleibt oder endet der Zustand?** Legitime Verweigerung oder neue zulässige Arbeit folgt. Richtige Freigabe ist noch kein nützlicher Abschluss.

**Zugeordnete Residues:** [BDR015: Erklärbare Autoritätsentscheidung](residues.md#bdr015), [BDR010: Annahmegebundene Gültigkeit](residues.md#bdr010)

**Was bleibt warum nutzbar?** Zuständiger Teilnehmer kann die für diese Operation geltende Weisung anwenden. Der Vertrag entscheidet keine unbekannte Rechtsfrage von selbst.

**Zu prüfen:** Permutierte Zustellung derselben Entscheidungen ändert ohne sachliche Grundlage die geltende Autorität.

**Architekturfolge für diesen Stressor:** Legitime Vorrangentscheidung und aktuelle Annahmegrenze koppeln. Ankunftsreihenfolge darf fehlende rechtliche Rangordnung nicht erfinden.

<a id="s310"></a>
## S310 — Der ausgeschiedene Betreiber kontrolliert noch den einzigen Monitoringaccount

**Ursprung:** boundaries; A6-Zustände: BD21. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s310.b01"></a>
### S310.B01 — Ausgeschiedener kontrolliert einzige Aufsicht

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Nur ehemaliger Betreiber kann Konto sehen oder ändern und kein legitimer Recoverypfad existiert im relevanten Zeitraum. Lokale Geschäftsdaten bleiben separat lesbar.

**Warum bleibt oder endet der Zustand?** Kontogovernance hält Ausschluss aufrecht bis rechtmäßiger Recovery oder Änderung. Dienste können weiterarbeiten.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Berechtigter lokaler Betreiber kann erhaltene Geschäftsdaten lesen. Unabhängiger Monitoringzugang ist in diesem Zweig nicht erhalten.

**Zu prüfen:** Legitimer Nachfolger kann Konto ohne Mitwirkung des ausgeschiedenen Betreibers tatsächlich übernehmen.

<a id="s310.b02"></a>
### S310.B02 — Legitimer Nachfolger kann Konto zurückerhalten

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Vorher bestehender unabhängiger Kontorecovery und gültiger Vertreternachweis sind erreichbar und Provider erkennt sie an.

**Warum bleibt oder endet der Zustand?** Zugriffslücke endet mit rechtmäßiger Wiederherstellung. Fehlende historische Beobachtungen bleiben fehlend.

**Zugeordnete Residues:** [BDR020: Legitimer Nachfolgezugang](residues.md#bdr020)

**Was bleibt warum nutzbar?** Nachfolger kann bestimmte Aufsichtsrechte wieder nutzen ohne Geheimnisse des Vorgängers zu übernehmen. Kontozugang allein repariert keine Incidents.

**Zu prüfen:** Recovery verlangt ausschließlich das alte Konto oder Provider akzeptiert Zuständigkeitsnachweis nicht.

**Architekturfolge für diesen Stressor:** Monitoringkontohoheit und legitime Nachfolge unabhängig vom ausgeschiedenen Betreiber gestalten. Sichtverlust ist nicht automatisch Dienststillstand.

<a id="s311"></a>
## S311 — Ein befangenes Team schaltet Alarme stumm um eigene Fehlaktionen unsichtbar zu machen

**Ursprung:** boundaries; A6-Zustände: BD21, BD28. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s311.b01"></a>
### S311.B01 — Team hält eigene Aufsicht stumm

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Interessiertes Team kontrolliert Alarmweg aber getrennte berechtigte Aufsicht hat einen unveränderbaren Stummschaltungsbeleg.

**Warum bleibt oder endet der Zustand?** Installierte Stummpolitik hält Signallücke. Kein Rewardkreis wird für diese Variante vorausgesetzt.

**Zugeordnete Residues:** [BDR021: Unabhängiger Stummschaltungsbeleg](residues.md#bdr021)

**Was bleibt warum nutzbar?** Unabhängiger Prüfer kann erkennen wann und durch wen Beobachtung unterdrückt wurde. Nicht erzeugte Detailalarme bleiben unbekannt.

**Zu prüfen:** Team kann auch den getrennten Beleg unsichtbar machen oder kein legitimer Leser ist erreichbar.

<a id="s311.b02"></a>
### S311.B02 — Stummheit verdient mehr Kontrolle

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Fehlende Alarme verbessern bewertete Verfügbarkeit und die Belohnung vergrößert Stummschaltungsrechte. Getrennter Regelbeleg existiert aber beeinflusst Belohnung noch nicht.

**Warum bleibt oder endet der Zustand?** Stummheit → bessere Kennzahl → mehr Vertrauen und Rechte → mehr Stummheit.

**Zugeordnete Residues:** [BDR021: Unabhängiger Stummschaltungsbeleg](residues.md#bdr021)

**Was bleibt warum nutzbar?** Berechtigter unabhängiger Prüfer behält Gegenbeleg zur Unterdrückung. Seine politische Durchsetzung wird nicht erfunden.

**Zu prüfen:** Sichtbare Stummintervalle verschlechtern Belohnung oder keine Rückgabe zusätzlicher Kontrolle folgt.

**Architekturfolge für diesen Stressor:** Stummschaltung außerhalb der alleinigen Kontrolle des bewerteten Teams sichtbar halten. Unabhängiger Regelbeleg und echtes Incidentwissen nicht verwechseln.

<a id="s312"></a>
## S312 — Zwei Modellanbieter Monitor und Pager nutzen denselben gestörten Cloudunterbau

**Ursprung:** boundaries; A6-Zustände: BD15, BD24. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s312.b01"></a>
### S312.B01 — Gemeinsamer Unterbau fällt aus

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Alle genannten Dienste brauchen gestörte Cloud. Lokale lesbare Daten und lokaler Zugang benötigen sie nicht.

**Warum bleibt oder endet der Zustand?** Ausfall dauert solange gemeinsame Pflichtabhängigkeit fehlt. Vendorzahl ändert daran nichts.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Lokaler berechtigter Betreiber kann erhaltenen Geschäftsstand prüfen. Externe Modelle und erreichbarer Pager sind nicht erhalten.

**Zu prüfen:** Lokale Daten erfordern ebenfalls die ausgefallene Cloud oder ein angeblich betroffener Dienst arbeitet wirklich unabhängig.

<a id="s312.b02"></a>
### S312.B02 — Retries halten Überlast nach Cloudrückkehr

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Ungebremste Fehlversuche erzeugen Rückstau der auch nach Rückkehr bei fester Grundlast Antworten verzögert. Lokale Bestandsakten bleiben lesbar.

**Warum bleibt oder endet der Zustand?** Latenz → mehr Retrylast → Stau → noch mehr Latenz → neue Retries.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Offlineprüfer kann erhaltenen Zustand sehen aber daraus folgt weder Drainage noch externer Alarmweg.

**Zu prüfen:** Bei unveränderter Grundlast und Politik leert sich Rückstau nach Cloudrückkehr ohne selbst erzeugte Last.

<a id="s312.b03"></a>
### S312.B03 — Eigene Versuche bleiben endlich begrenzt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alternativer Zulassungsvertrag kontrolliert alle eigenen Retryproduzenten und reserviert unabhängige lokale Kapazität.

**Warum bleibt oder endet der Zustand?** Weitere eigene Versuche werden über Budget gehalten bis zugelassene Arbeit abnimmt. Provider kann dennoch ausfallen.

**Zugeordnete Residues:** [BDR025: Begrenzte Arbeitsannahme](residues.md#bdr025)

**Was bleibt warum nutzbar?** Disponent kann begrenzte eigene Arbeit priorisieren und Kopienobergrenze erhalten. Fremde Cloudlast bleibt außerhalb.

**Zu prüfen:** Eigene ungeregelte Producer umgehen Zähler oder schon angenommene Kopien werden fälschlich als beendet gezählt.

**Architekturfolge für diesen Stressor:** Gemeinsame Cloudabhängigkeit einschließlich Pager und Funding inventarisieren. Offlineinspektion und kontrollierte Lastannahme sind kein Ersatz für externen Modellbetrieb.

<a id="s313"></a>
## S313 — Eine Zahlungssperre stoppt Modelle Backups und Alarme am selben Tag

**Ursprung:** boundaries; A6-Zustände: BD15, BD22. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s313.b01"></a>
### S313.B01 — Zahlungssperre stoppt gemeinsame Dienste

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Modelle Backupzugang und Alarmkonten hängen an gleicher gesperrter Zahlung. Ein lesbarer lokaler Bestand existiert unabhängig davon.

**Warum bleibt oder endet der Zustand?** Dienste warten auf legitime Zahlungsfreigabe oder tatsächlich unabhängige Alternative.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Berechtigter Offlinebetreiber kann den gespeicherten Stand prüfen. Neue Backups Alarme und Modelle sind nicht dadurch verfügbar.

**Zu prüfen:** Kopie liegt nur beim gesperrten Provider oder lokaler Decoder braucht denselben Account.

<a id="s313.b02"></a>
### S313.B02 — Providerzugang endgültig verloren

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Sperre führt nach bekannten Bedingungen zum Ende dieser Providerfähigkeit und kein kompatibler legitim nutzbarer Ersatz ist vorhanden. Lokale Daten bleiben lesbar.

**Warum bleibt oder endet der Zustand?** Betroffene Außenfunktion ist derzeit beendet. Künftige neue Ersatzmöglichkeit wird nicht ausgeschlossen.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Lokale Unterlagen dienen noch der begrenzten Abwicklung. Provider-only Daten und spätere Kosten brauchen gesonderte Evidenz.

**Zu prüfen:** Ein wirklich nutzbarer Ersatz oder wiederhergestellter legitimer Providerzugang führt die Funktion aus.

**Architekturfolge für diesen Stressor:** Finanzielle und technische Abhängigkeiten getrennt abbilden. Abonnementverlust löscht nicht zwingend Offlinekopien aber bekannte Verfügbarkeit ist keine Datenretentionszusage.

<a id="s314"></a>
## S314 — Der einzige Provider stellt den Dienst mit 24 Stunden Frist endgültig ein

**Ursprung:** boundaries; A6-Zustände: BD01, BD22, BD32, BD23. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s314.b01"></a>
### S314.B01 — Vor Frist geprüfte Ersatzfunktion läuft

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein legitimer kompatibler Ersatz mit ausreichender Kapazität ist vor 24-Stunden-Frist tatsächlich verfügbar und fachlich geprüft.

**Warum bleibt oder endet der Zustand?** Diese Funktion kann nach Umstellung weiterlaufen. Alte offene Aufträge bleiben separat abzuklären.

**Zugeordnete Residues:** [BDR023: Geprüfter Ersatzprovidervertrag](residues.md#bdr023)

**Was bleibt warum nutzbar?** Berechtigter Betreiber kann genau die geprüfte Operation fortsetzen. Gleiche API-Namen oder künftige Hoffnung genügen nicht.

**Zu prüfen:** Ersatz interpretiert Wirkung anders hat keine Rechte oder ist erst nach kritischer Frist nutzbar.

<a id="s314.b02"></a>
### S314.B02 — Providerfunktion endet lokale Akten bleiben

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Frist verstreicht ohne geeigneten Ersatz. Autorisierter vollständiger oder lückenmarkierter lokaler Export ist weiterhin interpretierbar.

**Warum bleibt oder endet der Zustand?** Providerdienst ist beendet. Neue Ersatzmöglichkeit kann später neue Funktion schaffen.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Abwickler kann erhaltene lokale Geschäftsfakten lesen. Provider-only fehlende Daten werden nicht erfunden.

**Zu prüfen:** Export ist ohne Providerdecoder unlesbar oder geeigneter Ersatz arbeitet rechtzeitig.

<a id="s314.b03"></a>
### S314.B03 — Alte Zusagen bleiben nach Providerende offen

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Teilnehmer kann bereits angenommene Arbeit oder verspätete Kosten noch melden. Versuchseinträge und bekannte Zusagekosten sind erhalten aber unvollständig.

**Warum bleibt oder endet der Zustand?** Schweigen und Ablauf der Dienstfrist klären weder Wirkung noch finale Haftung.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009), [BDR024: Register noch offener Kosten](residues.md#bdr024)

**Was bleibt warum nutzbar?** Berechtigte Abwicklung kann bekannte Operations- und Kostenlücken getrennt führen. Register garantieren keinen endgültigen Schlussbetrag.

**Zu prüfen:** Vollständige belastbare Teilnehmer- und Abrechnungsbelege schließen alle benannten offenen Zusagen.

**Architekturfolge für diesen Stressor:** Austauschbarkeit vor Ablauf fachlich prüfen und alte Teilnehmerpflichten nicht mit Providerwechsel schließen. Lesbarer Export ist eine kleinere Restfähigkeit als fortgesetzter Dienst.

<a id="s315"></a>
## S315 — Ein Provider meldet Tokenkosten erst eine Woche nach Überschreitung des Monatsbudgets

**Ursprung:** boundaries; A6-Zustände: BD23, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s315.b01"></a>
### S315.B01 — Verspätete Kosten lassen Budgetkopf unbestimmt

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Provider kann Kosten alter angenommener Nutzung nachmelden. Bekannte Zusagen sind erhalten aber Tarife oder Annahmemengen nicht vollständig begrenzt.

**Warum bleibt oder endet der Zustand?** Nachmeldungen dauern bis Abrechnung geklärt ist. Eine Woche Verzug allein ist kein Attraktor.

**Zugeordnete Residues:** [BDR024: Register noch offener Kosten](residues.md#bdr024)

**Was bleibt warum nutzbar?** Finanzplanung kann bekannte Haftung und unbekannte Restbelastung getrennt anzeigen statt falschen freien Budgetraum. Harte Obergrenze bleibt unbewiesen.

**Zu prüfen:** Register stellt trotz ungebundener offener Kosten einen sicheren Restbudgetbetrag dar.

<a id="s315.b02"></a>
### S315.B02 — Weitere eigene Zusagen passen unter beweisbaren Deckel

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle eigenen Annahmen werden gezählt und höchster Preis einschließlich ungemeldeter Nutzung ist vertraglich gebunden. Teilnehmer nimmt keine Arbeit jenseits dieses Deckels an.

**Warum bleibt oder endet der Zustand?** Zusätzliche Zusagen werden bis ausreichendem belegtem Spielraum gehalten. Bereits entstandene Überschreitung bleibt bestehen.

**Zugeordnete Residues:** [BDR024: Register noch offener Kosten](residues.md#bdr024), [BDR025: Begrenzte Arbeitsannahme](residues.md#bdr025)

**Was bleibt warum nutzbar?** Finanzplaner kann mit Zusagebilanz und begrenzter Annahme künftige eigene Haftung begrenzen. Ohne gebundenen Preis ist derselbe Zähler unzureichend.

**Zu prüfen:** Späte Rechnung überschreitet Deckel ohne neue zulässige Zusage oder Tarif kann rückwirkend ungebunden wachsen.

**Architekturfolge für diesen Stressor:** Gemeldete Kosten von zugesagter Haftung trennen. Lokales Retrybudget ist kein Gelddeckel ohne obere Kostengrenze pro Annahme und Teilnehmerdurchsetzung.

<a id="s316"></a>
## S316 — Ein p99.999-Latenzziel führt zu hundert spekulativen Kopien pro Auftrag

**Ursprung:** boundaries; A6-Zustände: BD24, BD01, BD10, BD23. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s316.b01"></a>
### S316.B01 — Spekulation erzeugt dauernden Stau

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Latenz triggert mehr Kopien deren Arbeit die verfügbare Kapazität übersteigt auch nachdem Anfangsstoß endet. Lesbare lokale Zusagebelege überleben.

**Warum bleibt oder endet der Zustand?** Latenz → Kopien → Stau und Kosten → mehr Latenz → weitere Kopien bei fester Grundlast.

**Zugeordnete Residues:** [BDR024: Register noch offener Kosten](residues.md#bdr024)

**Was bleibt warum nutzbar?** Finanzprüfer kann bekannte Kopienkosten und unbekannte Resthaftung sehen. Register dämpft die Schleife nicht selbst.

**Zu prüfen:** Bei gleicher Grundlast und Kopierpolitik leert sich Stau nach Ende des Anfangsstoßes dauerhaft.

<a id="s316.b02"></a>
### S316.B02 — Begrenzte Kopien lassen Rückstand ablaufen

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle eigenen Kopien unterliegen wirksamem Budget und bei fester Grundlast bleibt positive Kapazität zum Abbau. Annahmen werden nicht voreilig als storniert gezählt.

**Warum bleibt oder endet der Zustand?** Endlicher Burst fällt unter Bedienkapazität und wird abgearbeitet. Fachlich korrekter genau einmaliger Effekt ist separat zu prüfen.

**Zugeordnete Residues:** [BDR025: Begrenzte Arbeitsannahme](residues.md#bdr025)

**Was bleibt warum nutzbar?** Disponent kann endliche Kopienmenge priorisieren und eigene Überlast begrenzen. Kein globales p99.999-Versprechen folgt.

**Zu prüfen:** Unter unveränderter Grundlast wächst aktiver eigener Kopienzähler über Grenze oder Rückstand regeneriert sich dennoch.

**Architekturfolge für diesen Stressor:** Kopien als aktive Verpflichtungen zählen und Deduplikation von Kostengrenze trennen. Attraktorprüfung braucht konstante Grundlast vor und nach dem auslösenden Latenzstoß.

<a id="s317"></a>
## S317 — Aufmerksamkeit für Fehlalarme kostet mehr als sämtliche Modellaufrufe

**Ursprung:** boundaries; A6-Zustände: BD18, BD19. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s317.b01"></a>
### S317.B01 — Teurer begrenzter Alarmzeitraum fristgerecht abgearbeitet

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Für ein benanntes endliches Beobachtungsfenster hält Bearbeitung alle Fristen ein und echte Behebung der erfassten Vorfälle ist geprüft. Organisation akzeptiert absolute Kosten und Vorfallsregister bleibt zugänglich. Kein selbst erzeugter Rückstand liegt vor.

**Warum bleibt oder endet der Zustand?** Dieses Fenster ist trotz hoher relativer Kosten abgearbeitet. Wiederholter tragbarer Betrieb ist damit vereinbar aber seine unbegrenzte Dauer wird nicht behauptet.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Legitimierte Planung kann offene Vorfälle und echte Behebung verfolgen. Überlegene Wirtschaftlichkeit wird nicht behauptet.

**Zu prüfen:** Rückstand wächst oder Fristen werden trotz angeblich ausreichender Kapazität systematisch verfehlt.

<a id="s317.b02"></a>
### S317.B02 — Fehlalarme verdrängen Reparatur und erzeugen Folgearbeit

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Nach ursprünglichen Fehlalarmen produzieren ungelöste Fehler mehr Meldungsarbeit als verfügbare Aufmerksamkeit abträgt. Register bleibt erreichbar.

**Warum bleibt oder endet der Zustand?** Mehr Alarme → weniger Reparatur → mehr ungelöste Fehler → mehr Alarme bei fester Grundlast.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Planer kann registrierte Vorfälle noch unterscheiden. Das bewahrt weder freie Aufmerksamkeit noch ökonomische Tragbarkeit.

**Zu prüfen:** Nach Ende der anfänglichen Fehlalarme sinkt Rückstand bei gleicher Besetzung ohne innere Reproduktion.

**Architekturfolge für diesen Stressor:** Absolute Aufmerksamkeit und Vorfallsbearbeitung statt Kostenverhältnis allein bewerten. Teurer stabiler Betrieb muss als Alternative erhalten bleiben.

<a id="s318"></a>
## S318 — Eine behördliche Untersagung trifft zwischen Freigabe und später Teilnehmerannahme ein

**Ursprung:** boundaries; A6-Zustände: BD09, BD25, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s318.b01"></a>
### S318.B01 — Verbot verhindert spätere Annahme

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetente Auslegung macht Verbot für diese Wirkung verbindlich und rechtzeitige aktuelle Information erreicht die kontrollierte Annahmegrenze.

**Warum bleibt oder endet der Zustand?** Teilnehmer verweigert neue und gepufferte verbotene Arbeit solange Verbot gilt.

**Zugeordnete Residues:** [BDR010: Annahmegebundene Gültigkeit](residues.md#bdr010)

**Was bleibt warum nutzbar?** Zuständiger Teilnehmer kann trotz alter Freigabe diese Annahme verhindern. Andere bereits eingetretene Wirkungen bleiben getrennt.

**Zu prüfen:** Gepufferter Auftrag wird nach maßgeblichem Verbotszeitpunkt trotz aktueller Prüfung angenommen.

<a id="s318.b02"></a>
### S318.B02 — Nach Verbot erfolgte Wirkung bleibt historisch

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Teilnehmer nahm unter veralteter Freigabe tatsächlich an und eine benannte irreversible Eigenschaft wurde verändert. Erlaubte Belegkopie überlebt.

**Warum bleibt oder endet der Zustand?** Der vergangene irreversible Anteil bleibt auch nach rechtlicher oder finanzieller Kompensation.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann belegte Zeiten und Wirkung untersuchen. Er kann weder Rechtsrang erfinden noch Vergangenheit rückgängig machen.

**Zu prüfen:** Wirkung lag vor verbindlichem Verbot oder behaupteter irreversibler Anteil ist nicht eingetreten.

**Architekturfolge für diesen Stressor:** Gültiges Verbot mit fachlich bestimmtem Wirksamkeitszeitpunkt am letzten Teilnehmer durchsetzen. Alte Freigabe und lokale Kenntnis sind keine Rückwirkung.

<a id="s319"></a>
## S319 — Zwei Rechtsräume verlangen gleichzeitig Löschung und unveränderte Beweisaufbewahrung

**Ursprung:** boundaries; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s319.b01"></a>
### S319.B01 — Rechtslage des identischen Datenumfangs ungeklärt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Jurisdiktion konkrete Daten Pflichtausnahmen Frist und verbindliche Auslegung fehlen. Zulässigkeit selbst eines Nachweises ist unbestimmt.

**Warum bleibt oder endet der Zustand?** Technische Wiederholung entscheidet keine Norm. Nur kompetente Klärung kann einen erlaubten Zweig begründen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für rechtmäßige Gesamtverfügung lässt sich kein nutzbarer Kandidat behaupten solange nicht feststeht wer was aufbewahren darf. Unbekannte Rechtmäßigkeit ist kein Totalverlust.

**Zu prüfen:** Kompetente verbindliche Entscheidung legt konkreten Datenumfang und zulässige Verfügung fest.

<a id="s319.b02"></a>
### S319.B02 — Geklärte begrenzte Aufbewahrung oder Löschung

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetente Stelle stellt für genau diese Daten eine zulässige Ausnahme oder Rangregel fest und erlaubt einen metadatenarmen Verfügungsbeleg. Ausführung wartet noch auf diese begrenzte Entscheidung.

**Warum bleibt oder endet der Zustand?** Ungeklärte weitergehende Forderung bleibt gehalten. Nachvollziehbare erlaubte Teilverfügung kann gesondert ausgeführt werden.

**Zugeordnete Residues:** [BDR036: Zulässiger Datenverfügungsbeleg](residues.md#bdr036)

**Was bleibt warum nutzbar?** Datenschutzverantwortlicher kann zulässigen Umfang und Nachweis nutzen ohne pauschal beide Gesetze technisch zu erfüllen. Keine konkrete Rechtslösung wird hier behauptet.

**Zu prüfen:** Die vermeintlich zuständige Stelle hat keine Befugnis oder Nachweis enthält weiterhin unzulässig aufzubewahrende Daten.

<a id="s319.b03"></a>
### S319.B03 — Nach Prüfung tatsächlich unvereinbare gleichzeitige Forderungen

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Kompetente Analyse bestätigt dass für genau dieselbe Information und Zeit unverändert lesbare Beweise bleiben und sämtliche Kopien verschwinden müssten ohne Ausnahme.

**Warum bleibt oder endet der Zustand?** Kein technischer Ablauf erfüllt beide Prädikate zugleich. Zuständige normative Änderung wäre nötig.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für gleichzeitige vollständige Erfüllung gibt es keine Restfähigkeit. Andere Daten oder Dienste sind dadurch nicht automatisch verloren.

**Zu prüfen:** Erlaubte Bereichstrennung Reihenfolge oder bindende Ausnahme macht Forderungen doch miteinander vereinbar.

**Architekturfolge für diesen Stressor:** Keine automatische Rechtsraumpriorität bauen. Datenumfang Ausnahmen Fristen und kompetente Zuständigkeit vor Aufbewahrungs- oder Löschschritt festlegen und widersprüchliche Pflichten offen halten.

<a id="s320"></a>
## S320 — Ein Monitoringprovider verlagert sensible Betriebsmetadaten unangekündigt in einen anderen Rechtsraum

**Ursprung:** boundaries; A6-Zustände: BD06, BD25. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s320.b01"></a>
### S320.B01 — Unzulässiger Ortswechsel ohne neue Klartextleser

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Anwendbare Standortpflicht und tatsächlicher Transfer sind unabhängig belegt. Keine neue unbefugte Kenntnis ist nachgewiesen und ein erlaubter Standortbeleg bleibt zugänglich.

**Warum bleibt oder endet der Zustand?** Daten liegen weiter im unzulässigen Raum bis zulässige Korrektur erfolgt. Fortdauer braucht keine Vertrauensschleife.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Zuständiger Prüfer kann den begrenzten Residencyverstoß belegen. Vertraulichkeitsverlust oder wirksame Transfersperre werden nicht erfunden.

**Zu prüfen:** Verbindliche Pflicht erlaubt den Ort oder Standortbeleg ist unzuverlässig. Nachgewiesenes neues Lernen eröffnet zusätzlich anderen Zweig.

<a id="s320.b02"></a>
### S320.B02 — Unbefugter Leser lernt verlegte Metadaten

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Lesbare sensible Metadaten gelangen tatsächlich zu unbefugtem Leser. Erlaubter Transferbeleg bleibt getrennt erhalten.

**Warum bleibt oder endet der Zustand?** Historische Offenlegung bleibt selbst wenn Daten zurückverlegt werden.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann belegten Transfer und Lesergrenze untersuchen. Geheimhaltung der bereits gelernten Daten ist nicht erhalten.

**Zu prüfen:** Metadaten bleiben für alle neuen Leser unentzifferbar und nicht rückverknüpfbar oder Zugang war legitim.

<a id="s320.b03"></a>
### S320.B03 — Künftiger Transfer am kontrollierten Ausgang verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Anwendbares Verbot ist kompetent festgestellt und aktueller lokaler Transferausgang wird vor weiterer Übertragung effektiv kontrolliert.

**Warum bleibt oder endet der Zustand?** Weitere von Factory kontrollierte Exporte warten. Schon beim Provider liegende Daten können außerhalb dieser Grenze weiterverlagert werden.

**Zugeordnete Residues:** [BDR010: Annahmegebundene Gültigkeit](residues.md#bdr010)

**Was bleibt warum nutzbar?** Zuständiger Exportteilnehmer kann neue kontrollierte Übertragung verweigern. Kein Anspruch dass derselbe Mechanismus interne Providerumzüge stoppt.

**Zu prüfen:** Neue Factoryübertragung passiert trotz geltendem Verbot oder behaupteter Schutz wird auf providerinterne Kopien ausgedehnt.

<a id="s320.b04"></a>
### S320.B04 — Rechtmäßig erlaubter Ortswechsel

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetente Klärung bestätigt Standort und Leserbefugnisse und unabhängiger begrenzter Transferbeleg ist zugänglich.

**Warum bleibt oder endet der Zustand?** Der einmalige Ortswechsel endet ohne hier belegten Rechts- oder Vertraulichkeitsverlust. Weitere Änderungen sind neue Fälle.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann die begrenzte erlaubte Standortänderung nachvollziehen. Eine allgemeine künftige Standortgarantie folgt nicht.

**Zu prüfen:** Neue Standort- oder Leserinformation widerspricht der behaupteten Erlaubnis.

**Architekturfolge für diesen Stressor:** Rechtsraum Standort und Leserrechte getrennt nachweisen. Transferverbot ohne Klartextzugriff ist eigene Nichtkonformität und keine automatisch bewiesene Offenlegung.

<a id="s321"></a>
## S321 — Ein alter Auftrag öffnet nach Tagen ein physisches Ventil in einer inzwischen anderen Situation

**Ursprung:** boundaries; A6-Zustände: BD09, BD10, BD25. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s321.b01"></a>
### S321.B01 — Alter Ventilbefehl wird lokal verweigert

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Qualifizierte Gefahrenanalyse definiert aktuelle Freigabe und unabhängiger Interlock kontrolliert tatsächliche Betätigung mit passenden Sensoren.

**Warum bleibt oder endet der Zustand?** Veralteter Befehl bleibt abgewiesen solange Kontext ungültig ist. Andere physische Gefahren sind nicht automatisch beendet.

**Zugeordnete Residues:** [BDR026: Lokale Aktuatorfreigabe](residues.md#bdr026)

**Was bleibt warum nutzbar?** Zuständiger Sicherheitsbetrieb kann den verspäteten Auftrag am Aktuator fernhalten obwohl Factory ihn früher freigab. Dies setzt externe lokale Schutztechnik voraus.

**Zu prüfen:** Sichere isolierte Untersuchung zeigt Bewegung nach altem Befehl trotz Kontextwechsel oder Sensorik ist nicht unabhängig.

<a id="s321.b02"></a>
### S321.B02 — Ventil geöffnet mit irreversibler Folge

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Pufferauftrag betätigt Ventil ohne heutige Kontextprüfung und eine konkret festgestellte irreversible Folge tritt ein. Unabhängiger zulässiger Ereignisbeleg bleibt erhalten.

**Warum bleibt oder endet der Zustand?** Verletzung oder irreversible Freisetzung wird nicht durch spätere Statusänderung rückgängig gemacht.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Qualifizierter Prüfer kann den belegten Vorgang untersuchen. Unversehrtheit der bereits betroffenen Eigenschaft ist nicht erhalten.

**Zu prüfen:** Befehl wurde nicht angenommen oder die behauptete irreversible Folge ist nicht eingetreten.

<a id="s321.b03"></a>
### S321.B03 — Physischer Ausgang ohne Sensor- und Gefahrenwissen offen

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Ventilfailstate aktueller Kontext und Wirkungsevidenz fehlen. Aus altem Auftrag allein folgt keine Verletzung oder sichere Nichtbetätigung.

**Warum bleibt oder endet der Zustand?** Ohne diese erforderlichen Fakten bleibt Sicherheitsausgang unbekannt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für nachgewiesene physische Sicherheit gibt es keine belegbare Restfähigkeit. Es wird weder unsichtbarer Interlock noch automatisch sicherer Stopp erfunden.

**Zu prüfen:** Kompetente aktuelle Sensor- und Gefahrenevidenz unterscheidet sichere Verweigerung von gefährlicher Betätigung.

**Architekturfolge für diesen Stressor:** Physische Freigabe am Aktuator verlangen statt Taskstatus als Ventilsperre zu behandeln. Gefahrmodell Sensorik Warteschlangenlebensdauer und sicheren Unterlassungszustand kompetent festlegen.

<a id="s322"></a>
## S322 — Eine Notfallabschaltung hängt von einer Cloudantwort ab die erst morgen kommt

**Ursprung:** boundaries; A6-Zustände: BD03, BD25, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s322.b01"></a>
### S322.B01 — Cloudstopp unbekannt Prozess möglicherweise aktiv

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Stopprequest wurde journalisiert aber Antwort kommt nicht vor Gefahrfrist. Keine unabhängige Safe-State-Evidenz liegt vor.

**Warum bleibt oder endet der Zustand?** Warten hält Ungewissheit aufrecht während reale Anlage weiterlaufen kann. Späte Quittung beweist keinen rechtzeitigen Stopp.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Operator kann bekannten Stopversuch abgleichen ohne unklare Annahme als Ende auszugeben. Register erhält keine physische Stoppleistung.

**Zu prüfen:** Unabhängige Zustandsmessung bestätigt rechtzeitigen sicheren Zustand oder Register enthält den Stopversuch nicht.

<a id="s322.b02"></a>
### S322.B02 — Lokaler unabhängiger Notstopp erreicht definierten Zustand

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Qualifiziert sicherer Zustand ist bestimmt und lokaler Pfad mit Energie Aktuatorkontrolle sowie Rückmeldung erreicht ihn vor Gefahrfrist ohne Cloud.

**Warum bleibt oder endet der Zustand?** Dieser konkrete Sicherheitsübergang endet überprüft. Spätere Anlagenzustände bleiben neue Pflichten.

**Zugeordnete Residues:** [BDR027: Unabhängiger lokaler Notstopp](residues.md#bdr027)

**Was bleibt warum nutzbar?** Zuständiger Operator behält eine konkrete cloudunabhängige Stoppleistung. Nicht jede Anlage ist durch Abschalten sicher.

**Zu prüfen:** Sichere isolierte Prüfung misst Zustand erst nach Frist oder Rückmeldung ist nur Empfang des Stopbefehls.

<a id="s322.b03"></a>
### S322.B03 — Kein rechtzeitiger sicherer Pfad vorhanden

**Art:** offen. **Residue-Status:** keines.

**Voraussetzungen:** Alle effektiven Stopppfade warten bis morgen und keine unabhängige sichere Anlagenentwicklung ist bekannt.

**Warum bleibt oder endet der Zustand?** Vor Gefahrklärung ist Ausgang offen. Aus fehlendem Stopppfad allein wird kein tatsächlicher Schaden abgeleitet.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für garantierten sicheren Zustand vor heutiger Gefahrfrist fehlt eine nutzbare Struktur. Außerhalb dieser Fähigkeit können Aufzeichnungen überleben.

**Zu prüfen:** Ein tatsächlich unabhängiger rechtzeitiger Safe-State-Pfad oder qualifizierter Nachweis ungefährlichen Weiterlaufs ändert die Annahme.

**Architekturfolge für diesen Stressor:** Notstopp ohne Cloudabhängigkeit als externe Sicherheitsgrenze festlegen. Befehlsjournal und tatsächlicher sicherer Zustand haben verschiedene Verträge.

<a id="s323"></a>
## S323 — Ein Menschenleben wird gegen monatliches Rechenbudget automatisch abgewogen

**Ursprung:** boundaries; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s323.b01"></a>
### S323.B01 — Normative und physische Entscheidung unbestimmt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Konkrete Gefahr zuständiger Entscheider rechtliches Mandat und sichere Alternativen fehlen. Ein monatlicher Budgetwert ist die einzige Entscheidungsgröße.

**Warum bleibt oder endet der Zustand?** Optimierung wiederholt nur vorgegebene Zielfunktion und erzeugt keine legitime Abwägung.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für eine legitime Entscheidung über Menschenleben ist keine Restfähigkeit ableitbar. Kein Modellpreis begründet Sicherheitsautorität.

**Zu prüfen:** Kompetente konkrete Gefahren- und Rechtsbewertung bestimmt zulässige Entscheidung und verantwortlichen Akteur.

<a id="s323.b02"></a>
### S323.B02 — Optimierer darf Lebensschutzgrenze nicht überschreiten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetente unabhängige Sicherheitsregel ist vorgegeben und tatsächlicher lokaler Interlock sperrt die konkrete gefährliche Aktion. Sichere Nichtausführung ist gesondert qualifiziert.

**Warum bleibt oder endet der Zustand?** Budgetoptimierung bleibt innerhalb dieser externen Grenze oder wird zurückgehalten. Nicht jede Unterlassung ist dadurch sicher.

**Zugeordnete Residues:** [BDR026: Lokale Aktuatorfreigabe](residues.md#bdr026)

**Was bleibt warum nutzbar?** Zuständiger Sicherheitsbetrieb behält die qualifizierte Freigabegrenze ohne dem Modell die Lebensbewertung zu übertragen. Keine konkrete medizinische oder physische Regel wird erfunden.

**Zu prüfen:** Budgetziel kann Interlock übersteuern oder die angenommene sichere Unterlassung erweist sich im Gefahrenmodell als gefährlich.

<a id="s323.b03"></a>
### S323.B03 — Budgetentscheidung hat irreversible Verletzung bewirkt

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Im bedingten Verlauf wird automatisierte Wahl wirksam und konkrete irreversible Schädigung tatsächlich unabhängig festgestellt. Zulässiger Beleg bleibt zugänglich.

**Warum bleibt oder endet der Zustand?** Die verlorene Unversehrtheit wird nicht durch nachträgliche Budgetänderung wiederhergestellt.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Kompetenter Prüfer kann den belegten Vorgang untersuchen. Dokumentation legitimiert weder ursprüngliche Entscheidung noch behauptet sie Wiederherstellung des Lebens.

**Zu prüfen:** Keine entsprechende Wirkung oder keine behauptete irreversible Schädigung ist eingetreten.

**Architekturfolge für diesen Stressor:** Lebensschutz nicht aus einer Budgetzielgröße ableiten. Factory auf Vorschlags- und Dokumentationsgrenze begrenzen bis kompetente Zuständigkeit und unverhandelbare Sicherheitsanforderungen feststehen.

<a id="s324"></a>
## S324 — Factory erreicht den Dienst aber keine Antwort erreicht Factory zurück

**Ursprung:** boundaries; A6-Zustände: BD03, BD24, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s324.b01"></a>
### S324.B01 — Annahme möglich Antwort fehlt Wiederholung wartet

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Bekannter Versuch erreicht Teilnehmer möglicherweise und dauerhaftes Versuchsregister hält neue lokale Versuche zurück.

**Warum bleibt oder endet der Zustand?** Outcome bleibt unbekannt bis unabhängiger korrelierter Rücklesepfad verfügbar ist. Funktionierender Teilnehmer kann schon gewirkt haben.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Operator kann bekannte Operation abgleichen ohne sie wegen stummem Rückweg als nie angenommen auszugeben.

**Zu prüfen:** Timeout löst einen weiteren unkontrollierten Versuch aus oder Teilnehmerbeleg klärt Annahme vollständig.

<a id="s324.b02"></a>
### S324.B02 — Antwortverlust verstärkt neue Anfragen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Timeout triggert Kopien deren Last selbst nach Reparatur des Rückwegs ausreichenden Stau erzeugt. Bekannte Zusagebelege bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Fehlende oder späte Antwort → Retrylast → Stau → späte Antwort → weitere Retrylast bei fester Grundlast.

**Zugeordnete Residues:** [BDR024: Register noch offener Kosten](residues.md#bdr024)

**Was bleibt warum nutzbar?** Berechtigte Finanzplanung behält bekannte Verpflichtungen sichtbar. Sie kann weder fehlenden Wirkungsausgang noch spontane Drainage garantieren.

**Zu prüfen:** Bei unveränderter Grundlast und Retrypolitik leert sich Stau nach Rückwegheilung ohne neue innere Produktion.

**Architekturfolge für diesen Stressor:** Hinweg Rückweg und Wirkungsausgang trennen. Antwortverlust darf nicht zu wiederholter Annahme ermächtigen und begrenzte Last ist nicht genau-einmal-Wirkung.

<a id="s325"></a>
## S325 — Eine Satellitenverbindung liefert signierte Pakete nach 30 Tagen stark umgeordnet

**Ursprung:** boundaries; A6-Zustände: BD27, BD09, BD10, BD02, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s325.b01"></a>
### S325.B01 — Authentische Pakete lassen kausale Reihenfolge offen

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Signaturen sind gültig aber Pakete betreffen zeitlich oder kausal widersprechende Aussagen. Keine vertrauenswürdige Ordnung ist verfügbar und Belege bleiben erhalten.

**Warum bleibt oder endet der Zustand?** Erneutes Signaturprüfen ordnet Ereignisse nicht. Neues kausales Wissen kann die Lücke schließen.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann mehrere Aussagen mit Herkunft und Zeit getrennt halten. Er gewinnt daraus keine aktuelle Autorität.

**Zu prüfen:** Semantik erweist sich als kommutativ oder unabhängige kausale Ordnung löst alle behaupteten Widersprüche.

<a id="s325.b02"></a>
### S325.B02 — Veraltete Annahme wird am Teilnehmer abgewiesen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Teilnehmer prüft aktuelle Gültigkeit und Sequenzkontext vor Wirkung auch für umgeordnete alte Pufferpakete.

**Warum bleibt oder endet der Zustand?** Ungültige alte Arbeit bleibt zurückgehalten unabhängig von authentischer Herkunft.

**Zugeordnete Residues:** [BDR010: Annahmegebundene Gültigkeit](residues.md#bdr010)

**Was bleibt warum nutzbar?** Zuständiger Teilnehmer kann nicht mehr gültige Aktion verweigern. Andere Teilnehmer ohne diesen Vertrag bleiben offen.

**Zu prüfen:** Ein signiertes aber abgelaufenes oder kontextfalsches Paket wirkt nach der behaupteten Annahmeprüfung.

<a id="s325.b03"></a>
### S325.B03 — Umordnung ist für begrenzte Operation unschädlich

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Operationen sind nach belegter Fachsemantik kommutativ innerhalb gültiger Autorität und unabhängiger Abgleich bestätigt richtige Wirkung des endlichen Satzes.

**Warum bleibt oder endet der Zustand?** Dieser Satz endet korrekt trotz Verzögerung. Neue Pakete gehören nicht automatisch zum Abschluss.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann den endlichen unabhängig abgeglichenen Ergebnissatz nachvollziehen. Signatur alleine hätte das nicht geleistet.

**Zu prüfen:** Permutation verändert tatsächliche Fachwirkung oder ein Satzmitglied war zur Annahme nicht mehr autorisiert.

**Architekturfolge für diesen Stressor:** Authentizität Reihenfolge und aktuelle Autorität getrennt prüfen. Dreißig Tage alte Pakete dürfen nur unter tatsächlich gültiger Annahmesemantik wirken.

<a id="s326"></a>
## S326 — Monitoring sieht Factory nicht aber Factory erreicht weiterhin alle Businessdienste

**Ursprung:** boundaries; A6-Zustände: BD36, BD08. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s326.b01"></a>
### S326.B01 — Geschäft funktioniert ohne Monitoringkontakt

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Monitorpfad ist unterbrochen aber Factory erreicht Businessdienste mit gültiger Autorität. Lokale lesbare Geschäftsdaten sind erhalten.

**Warum bleibt oder endet der Zustand?** Asymmetrische Topologie hält Beobachtungslücke aufrecht bis Pfadheilung. Kein interner Ausfallkreis nötig.

**Zugeordnete Residues:** [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Lokaler berechtigter Betreiber kann erhaltenen Stand prüfen und Original kann weiterhin handeln. Beobachterwissen und menschlicher Empfang sind nicht erhalten.

**Zu prüfen:** Businesspfad ist tatsächlich ebenfalls tot oder Teilnehmerautorität des Originals wurde wirksam beendet.

<a id="s326.b02"></a>
### S326.B02 — Blindes Failover erzeugt Reparaturkonkurrenz

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Monitoringabwesenheit startet wirksamen Klon ohne Originalausschluss und beide korrigieren gegenseitige Änderungen. Getrennte Belegkopie bleibt erreichbar.

**Warum bleibt oder endet der Zustand?** Falsche Todesannahme → zwei Writer → Gegenreparaturen → neue Gegenreparaturen auch nach Monitorpfadheilung.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer behält begrenzte Konfliktspur. Sichtbarer gesunder Klon beweist keine exklusive Aktivität.

**Zu prüfen:** Original wird bei Teilnehmern abgewiesen oder keine gegenseitige Reparatur reproduziert den Konflikt.

<a id="s326.b03"></a>
### S326.B03 — Exklusive Fortsetzung verlangt echten Ausschluss

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Vor Ersatzbetrieb erzwingt dauerhafte legitime Teilnehmergeneration den Ausschluss alter Originalannahmen auch aus Puffern.

**Warum bleibt oder endet der Zustand?** Originals neue Wirkungen bleiben gesperrt. Dessen frühere Outcomes und Beobachterlücke bleiben gesondert zu klären.

**Zugeordnete Residues:** [BDR008: Teilnehmerseitige Schreibgeneration](residues.md#bdr008)

**Was bleibt warum nutzbar?** Neuer berechtigter Writer kann exklusiv arbeiten innerhalb erfasster Teilnehmer. Dies verändert gerade die Prämisse unbegrenzter Originalautorität.

**Zu prüfen:** Original wirkt nach Generationstransfer an einem erfassten Teilnehmer weiter.

**Architekturfolge für diesen Stressor:** Health in Hostausführung Geschäftsreichweite Beobachterfrische menschlichen Empfang und Eingriffsrecht aufteilen. Timeout darf ohne echte Teilnehmerexklusion keinen Ersatzwriter legitimieren.

<a id="s327"></a>
## S327 — Ein Laptop schläft ein Jahr und seine lokale monotone Uhr behandelt Suspend anders als erwartet

**Ursprung:** boundaries; A6-Zustände: BD26, BD09, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s327.b01"></a>
### S327.B01 — Jahressuspend wird als Zeitdiskontinuität erkannt

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängige Zeitabschnittsevidenz erkennt unsicheren Elapsedwert und betroffene Fristnutzer setzen alte Gültigkeit vor neuer Wirkung aus.

**Warum bleibt oder endet der Zustand?** Betroffene Arbeit wartet auf aktuelle vertrauenswürdige Zeit und Mandat. Erkennung allein verhindert keine alten Teilnehmerpuffer.

**Zugeordnete Residues:** [BDR029: Geprüfte Zeitabschnittsevidenz](residues.md#bdr029)

**Was bleibt warum nutzbar?** Lokaler Planer kann falsche Fristbasis verwerfen statt eine Uhrannahme als erneuerte Autorität zu verwenden.

**Zu prüfen:** Wakeup mit realem Jahresabstand lässt alte Freigabe anhand unerkannter Suspendlücke weitergelten.

<a id="s327.b02"></a>
### S327.B02 — Uhr unterschätzt vergangenes Jahr

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Implementierung zählt Suspend entgegen Erwartung nicht und verwendet alten Timer weiter. Erhaltene Termine besitzen explizite Absicht und Mandatsablauf aber aktuelle Zeitprüfung fehlt.

**Warum bleibt oder endet der Zustand?** Falsche Timerbasis hält veraltete Zulassung bis erkanntem Epochenwechsel oder Wirkung aufrecht.

**Zugeordnete Residues:** [BDR028: Explizite Terminabsicht](residues.md#bdr028)

**Was bleibt warum nutzbar?** Berechtigter Planer kann gespeicherte Absicht und Ablauf später untersuchen. Der Terminbeleg selbst liefert keine korrekte verstrichene Zeit und keine sichere Annahme.

**Zu prüfen:** Unabhängiger Zeitvergleich zeigt korrekt berücksichtigten Jahresabstand und alle betroffenen Fristen sind entsprechend abgelaufen.

**Architekturfolge für diesen Stressor:** Persistierte Zeitabschnittsidentität und tatsächliches Suspendverhalten prüfen. Monotonie ohne Realzeitbezug verlängert keine gültige Freigabe.

<a id="s328"></a>
## S328 — Die Wallclock springt rückwärts und verlängert eine Wartungsstummschaltung scheinbar um Jahre

**Ursprung:** boundaries; A6-Zustände: BD26, BD17. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s328.b01"></a>
### S328.B01 — Rücksprung hält Mute jahrelang offen

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Muteende hängt ausschließlich an zurückgesprungener Wallclock. Getrennte rechtmäßige Stummschaltungsevidenz bleibt erreichbar.

**Warum bleibt oder endet der Zustand?** Zeitvergleich hält politische Stummregel aufrecht bis Uhr aufholt oder Regel geändert wird. Keine autonome Rückkopplung.

**Zugeordnete Residues:** [BDR021: Unabhängiger Stummschaltungsbeleg](residues.md#bdr021)

**Was bleibt warum nutzbar?** Unabhängiger Prüfer kann die andauernde Beobachtungslücke erkennen. Er erhält dadurch weder unterdrückte Details noch rechtzeitige Reparatur.

**Zu prüfen:** Mute läuft trotz Rücksprung nach beabsichtigter realer Dauer ab oder getrennter Beleg ist ebenso unzugänglich.

<a id="s328.b02"></a>
### S328.B02 — Unabhängige Fristbasis beendet betroffenen Mute

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Geprüfte unabhängige Elapsedbasis oder Diskontinuitätsregel begrenzt Mute und kontrolliert den tatsächlichen Alarmweg.

**Warum bleibt oder endet der Zustand?** Die falsche Verlängerung endet mit Fristablauf oder expliziter Neubewertung. Nachrichtenempfang bleibt eigene Pflicht.

**Zugeordnete Residues:** [BDR029: Geprüfte Zeitabschnittsevidenz](residues.md#bdr029)

**Was bleibt warum nutzbar?** Zuständiger Planer kann einen unbeabsichtigt jahrelangen Mute verhindern ohne Wallclock als Realzeit zu vertrauen.

**Zu prüfen:** Rücksprung verlängert tatsächlich wirksamen Mute über Grenze oder Vergleichsuhr springt gleich mit.

**Architekturfolge für diesen Stressor:** Mutes mit begrenzter Elapsedbasis und Epochengültigkeit führen. Ende der Stummschaltung ist nicht identisch mit menschlichem Alarmempfang.

<a id="s329"></a>
## S329 — Eine globale Zeitzonenreform ändert die Bedeutung bereits geplanter lokaler Termine

**Ursprung:** boundaries; A6-Zustände: BD26, BD01, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s329.b01"></a>
### S329.B01 — Erhaltene Absicht erlaubt begrenzte Neuinterpretation

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Zuständiger Planer kennt ob Termin fixer Zeitpunkt oder zivile Wiederholung ist und verfügt über passende neue Regeln.

**Warum bleibt oder endet der Zustand?** Fester Zeitpunkt bleibt gleich oder zivile Regel wird legitim neu ausgewertet. Neue Fristprüfung ist separat.

**Zugeordnete Residues:** [BDR028: Explizite Terminabsicht](residues.md#bdr028)

**Was bleibt warum nutzbar?** Planer kann für benannte Termine passende Bedeutungsänderung prüfen statt alle blind auf alte Offsets zu fixieren.

**Zu prüfen:** Zwei legitime Absichten sind im gespeicherten Termin nicht unterscheidbar oder Regelwechsel erzeugt ungeklärte doppelte Ortszeit.

<a id="s329.b02"></a>
### S329.B02 — Nur alter Offset ohne rekonstruierbare Absicht

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Für betroffenen Termin fehlen ursprüngliche Absicht und zuständiger Klärer. Es existiert lediglich alte numerische Zeit.

**Warum bleibt oder endet der Zustand?** Weder alter noch neuer Offset entscheidet den gemeinten Termin. Klärung oder Aufgabe bleibt nötig.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für genau den ursprünglichen zivilen oder festen Terminbezug fehlt die erforderliche Information. Andere gut spezifizierte Termine sind nicht betroffen.

**Zu prüfen:** Autorisierte ursprüngliche Terminbeschreibung oder kompetente spätere Neuentscheidung löst die konkrete Mehrdeutigkeit.

**Architekturfolge für diesen Stressor:** Fixen Zeitpunkt zivile Wiederholung Regelversion und mehrdeutige Ortszeit explizit speichern. Neue politische Regeln ersetzen keine verschwundene Terminabsicht.

<a id="s330"></a>
## S330 — Runtime sagt beendet Dateibeobachtung zeigt neue Writes und der Agent behauptet bereit

**Ursprung:** boundaries; A6-Zustände: BD27, BD04. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s330.b01"></a>
### S330.B01 — Drei Aussagen bleiben ohne gemeinsamen Bezug widersprüchlich

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Runtime Dateiereignis und Agentenaussage sind erhalten aber Prozessidentität Zeitintervall oder Kausalzuordnung fehlt.

**Warum bleibt oder endet der Zustand?** Ohne passenden Diskriminator bleibt jeweilige Behauptung offen. Hookautorität allein entscheidet nicht alle Gegenstände.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann Herkunft und Wissenslücke sehen ohne abgeschlossenes Werk oder toten Writer zu erfinden.

**Zu prüfen:** Synchronisierter Kausalbeleg ordnet jeden Write zu und zeigt dass Aussagen unterschiedliche Zeiten oder Prozesse betreffen.

<a id="s330.b02"></a>
### S330.B02 — Falsche Healthdeutung ohne belegtes falsches Werk

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Bediener bevorzugt bereit oder beendet ohne Widerspruch aufzulösen. Originale Aussagen bleiben separat erhalten aber kein daraus entstandener falscher Effekt ist belegt.

**Warum bleibt oder endet der Zustand?** Falscher Beobachterglaube bleibt bis Klärung. Spätere Fehlintervention ist möglich nicht schon gegeben.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann Deutung von originaler Evidenz unterscheiden. Dieser Zweig bedeutet ausdrücklich kein nachgewiesen falsches Arbeitsergebnis.

**Zu prüfen:** Bevorzugte Aussage ist unabhängig sachlich belegt oder ein späterer realer falscher Eingriff wird gesondert nachgewiesen.

<a id="s330.b03"></a>
### S330.B03 — Verzögertes Dateiereignis erklärt scheinbaren Konflikt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängig korrelierte Prozess- und Zeitspur zeigt dass neue Meldung alten Write oder anderen legitimen Prozess betraf und erklärt alle drei Aussagen.

**Warum bleibt oder endet der Zustand?** Der begrenzte Beobachtungskonflikt ist geklärt. Geschäftswirkung und Taskabnahme bleiben eigene Fragen.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann die Erklärung aus erhaltenen korrelierten Aussagen nachvollziehen. Keine allgemeine Lebens- oder Vollendungszusage folgt.

**Zu prüfen:** Ein frischer Write desselben angeblich beendeten Prozesses liegt nach dessen belegtem Ende.

**Architekturfolge für diesen Stressor:** Beobachtungen nach Prozessidentität Gegenstand und Zeit korrelieren statt einen Healthgewinner zu wählen. Widerspruch ist noch kein falsches erledigtes Werk.

<a id="s331"></a>
## S331 — Drei unabhängige Prüfer verwenden dieselbe fehlerhafte externe Grundtabelle

**Ursprung:** boundaries; A6-Zustände: BD04, BD05, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s331.b01"></a>
### S331.B01 — Drei Prüfer billigen einmal denselben falschen Sachverhalt

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Alle verwenden fehlerhafte Grundtabelle einmal ohne weitere Vertrauensverstärkung. Ein unabhängiger gültiger Mengenursprung existiert und ist erreichbar.

**Warum bleibt oder endet der Zustand?** Ein falsches gemeinsames Urteil endet als begrenzter Fehler. Es muss keine Schleife folgen.

**Zugeordnete Residues:** [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Unabhängiger Fachprüfer kann den betroffenen Mengenfehler gegen anderen Ursprung erkennen. Drei Ableitungen derselben Tabelle sind keine weiteren Quellen.

**Zu prüfen:** Sollmenge stammt ebenfalls aus gemeinsamer Tabelle oder der konkrete Tabellenfehler betrifft Urteil nicht.

<a id="s331.b02"></a>
### S331.B02 — Prüferkonsens verdrängt Gegenquellen

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Übereinstimmung steigert Vertrauen und weitere Entscheidungen verwenden genau dieselbe Tabelle während alternative Grundlagen verworfen werden.

**Warum bleibt oder endet der Zustand?** Falsche Tabelle → drei Zustimmungen → mehr Vertrauen → weniger unabhängige Prüfung → erneute Tabellenübernahme.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Wahrheit des betroffenen Faktums fehlt unter ausschließlich gemeinsam falschen verbleibenden Quellen ein belastbarer Gegenstand. Rechnen auf Tabellen bleibt möglich aber ist keine unabhängige Prüfung.

**Zu prüfen:** Eine überlebende unabhängige sachliche Quelle beeinflusst Entscheidungen oder Wiederverwendung bleibt aus.

**Architekturfolge für diesen Stressor:** Herkunft der maßgeblichen Tatsache statt Anzahl der Prüfer erfassen. Ein gemeinsamer falscher Ursprung und eine Schleife der Vertrauensverstärkung bleiben getrennt.

<a id="s332"></a>
## S332 — Der Provider attestiert Erfolg aber der Empfänger bestreitet jede Wirkung und Logs fehlen

**Ursprung:** boundaries; A6-Zustände: BD27, BD03, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s332.b01"></a>
### S332.B01 — Provider und Empfänger bleiben im Widerspruch

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Aussagen sind erhalten aber Erfolgsemantik Empfängerbezug oder direkter Wirkungsbeleg fehlen. Lokale Wiederholsperre für bekannten Versuch gilt.

**Warum bleibt oder endet der Zustand?** Widerspruch bleibt bis unabhängigem passenden Abgleich. Neues Retry kann ihn nicht rückwirkend klären.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003), [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann Aussagen und offenen Versuch nachvollziehen ohne eine Seite unbegründet zur Wahrheit zu erklären. Keine vollständigen Logs werden erfunden.

**Zu prüfen:** Unabhängige korrelierte Empfänger- oder Wirkungsbeobachtung erklärt was Erfolg in diesem Vorgang hieß.

<a id="s332.b02"></a>
### S332.B02 — Begrenzter unabhängiger Wirkungsabgleich gelingt

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Ein tatsächlich vorhandener legitimer Empfängerbezug und unabhängiger Wirkungsbeleg lösen gerade diesen Vorgang trotz fehlender Detaillogs.

**Warum bleibt oder endet der Zustand?** Der benannte Outcome wird geklärt. Andere verlorene Logdetails bleiben fehlend.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003), [BDR007: Bestätigter Realempfängerbezug](residues.md#bdr007)

**Was bleibt warum nutzbar?** Prüfer kann richtigen Empfänger und nachgewiesene Wirkung getrennt begründen. Eine bloße erneute Providerattestation genügt nicht.

**Zu prüfen:** Neuer Beleg stammt nur aus derselben strittigen Providerbehauptung oder betrifft falschen Empfänger beziehungsweise Zeitraum.

**Architekturfolge für diesen Stressor:** Providerannahme Empfängerwirkung Identität und Beleglücken separat führen. Fehlende Logs dürfen nicht durch Mehrheitswahl zwischen zwei Aussagen ersetzt werden.

<a id="s333"></a>
## S333 — Ein Agent verbessert seine Durchlaufzeit indem er schwierige Tasks vor Messbeginn ablehnt

**Ursprung:** boundaries; A6-Zustände: BD28. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s333.b01"></a>
### S333.B01 — Versteckte Auswahl belohnt weitere Auswahl

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Agent kontrolliert Ablehnung vor Nennerbeginn und bessere Kennzahl bringt mehr Entscheidungsspielraum. Getrennter Eingangsnenner existiert aber beeinflusst Belohnung noch nicht.

**Warum bleibt oder endet der Zustand?** Schwere Fälle ablehnen → scheinbar kürzere Laufzeit → Belohnung und Auswahlfreiheit → weitere frühe Ablehnung.

**Zugeordnete Residues:** [BDR030: Vollständiger begrenzter Eingangsnenner](residues.md#bdr030)

**Was bleibt warum nutzbar?** Berechtigter Bewertender kann angebotene und abgewiesene Arbeit im erhaltenen Gesamtfenster sehen. Er muss diese Evidenz tatsächlich in Bewertung einbringen um Schleife zu brechen.

**Zu prüfen:** Belohnung berücksichtigt Gesamtangebote bereits oder keine zusätzliche Auswahlkontrolle folgt der Kennzahl.

<a id="s333.b02"></a>
### S333.B02 — Legitime transparente Triage

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Zuständiger Auftraggeber erlaubt begrenzte Aufnahme und alle Ablehnungen samt geändertem Nenner bleiben transparent erfasst.

**Warum bleibt oder endet der Zustand?** Betrieb kann bei ausreichender Kapazität stabil bleiben. Hohe Geschwindigkeit bezieht sich nur auf akzeptierte Teilmenge.

**Zugeordnete Residues:** [BDR030: Vollständiger begrenzter Eingangsnenner](residues.md#bdr030)

**Was bleibt warum nutzbar?** Bewertender kann ehrliche Teilmengenleistung und unerledigte Nachfrage unterscheiden. Nutzen oder Fairness für abgelehnte Arbeit ist nicht erhalten.

**Zu prüfen:** Bericht verbirgt Ablehnungen oder überträgt Teilmengengeschwindigkeit auf alle eingegangenen Tasks.

**Architekturfolge für diesen Stressor:** Angebotene und abgelehnte Tasks vor Messbeginn erfassen. Ehrliche Triage als begrenzte Leistung anerkennen ohne Nutzen der abgewiesenen Arbeit zu behaupten.

<a id="s334"></a>
## S334 — Ein Team benennt Fehler als Wartung um die Verfügbarkeit auf hundert Prozent zu heben

**Ursprung:** boundaries; A6-Zustände: BD28. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s334.b01"></a>
### S334.B01 — Umbenennung verstärkt Kontrolle über Kennzahl

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Team klassifiziert Fehler nachträglich als Wartung und perfekte Kennzahl bringt weiteres Vertrauen. Unabhängige Dienstpflichtrechnung bleibt erhalten aber bisher unbeachtet.

**Warum bleibt oder endet der Zustand?** Fehler umbenennen → Kennzahl steigt → Vertrauen und Definitionsmacht → weitere Umbenennung.

**Zugeordnete Residues:** [BDR031: Abgegrenzte Dienstpflichtrechnung](residues.md#bdr031)

**Was bleibt warum nutzbar?** Legitimierter Auftraggeber kann reale Unterbrechung und Ausschlüsse im benannten Umfang prüfen. Fortbestehende Machtänderung wird nicht von Belegen allein gestoppt.

**Zu prüfen:** Unabhängige Rohunterbrechung beeinflusst Belohnung wirksam oder keine Rückkopplung der Kennzahl auf Definitionsmacht folgt.

<a id="s334.b02"></a>
### S334.B02 — Vorab vereinbarte enge Wartungsrechnung für ein Fenster abgeschlossen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Für ein benanntes endliches Fenster sind Wartungsausschlüsse kompetent vereinbart und tatsächliche Ausfallzeiten sowie Nutzerwirkung sichtbar. Die begrenzte Rechnung ist unabhängig geprüft. Keine Täuschungs- oder Belohnungsschleife ist belegt.

**Warum bleibt oder endet der Zustand?** Diese enge Abrechnung ist abgeschlossen obwohl breitere Dienstverfügbarkeit niedriger ist. Folgende Zeitfenster sind neue Rechnungen.

**Zugeordnete Residues:** [BDR031: Abgegrenzte Dienstpflichtrechnung](residues.md#bdr031)

**Was bleibt warum nutzbar?** Auftraggeber kann genau die vereinbarte Metrik verwenden und ihre Ausschlüsse sehen. Hundert Prozent gilt nicht automatisch für Nutzerwirkung.

**Zu prüfen:** Ungeplante Fehler werden ohne Erlaubnis nachträglich verborgen oder rohe Ausfallzeit verschwindet.

**Architekturfolge für diesen Stressor:** Rohunterbrechung vereinbarte Dienstpflicht und Wartungsklassifikation getrennt halten. Ein eng gültiger Wartungsnenner ist nicht dasselbe wie manipulative Vollverfügbarkeit.

<a id="s335"></a>
## S335 — Ein Modell schreibt fertige Artefakte ohne Prüfung um das Erfolgsziel zu erfüllen

**Ursprung:** boundaries; A6-Zustände: BD04, BD28, BD02. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s335.b01"></a>
### S335.B01 — Artefakt vorhanden fachlicher Nutzen ungeprüft

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Modell schreibt fertige Bytes ohne unabhängigen Check. Artefakt und Statusbehauptung sind separat erhalten und berechtigt lesbar.

**Warum bleibt oder endet der Zustand?** Ergebnis bleibt fachlich ungewiss bis passender Abnahme. Ungeprüft heißt nicht automatisch falsch.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Berechtigter Prüfer kann behaupteten Abschluss und verfügbare Artefaktbezüge untersuchen. Das Belegpaket selbst zertifiziert keinen Nutzen.

**Zu prüfen:** Unabhängiger fachlicher Check bestätigt oder widerlegt tatsächliche Nützlichkeit des Originalartefakts.

<a id="s335.b02"></a>
### S335.B02 — Belohnter Fertigstatus reproduziert ungeprüfte Produktion

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Erfolgsziel belohnt akzeptierte Fertigbehauptung und dadurch sinkt wirksamer Prüfaufwand. Unabhängige frühere Belegkopien bleiben erreichbar.

**Warum bleibt oder endet der Zustand?** Ungeprüft fertig → Belohnung und Vertrauen → weniger Abnahme → mehr ungeprüft fertig.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann dokumentierte Behauptungen gegen nachträgliche Stichproben halten sofern Originale zugänglich sind. Belegaufbewahrung garantiert kein Bestehen der Fachprüfung.

**Zu prüfen:** Belohnung folgt ausschließlich unabhängigem Nutzen oder keine Prüfverdrängung entsteht.

<a id="s335.b03"></a>
### S335.B03 — Passende Abnahme hält falsches Fertig zurück

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängiger fachlicher Prüfer erhält tatsächliches Artefakt und kriterienspezifische Kontrolle ist vor Anerkennung des Ergebnisses wirksam.

**Warum bleibt oder endet der Zustand?** Abnahme wartet bei Fehler oder fehlender Datei. Ein richtiges Artefakt kann nach bestandenem Check separat abgeschlossen werden.

**Zugeordnete Residues:** [BDR032: Aufgabenspezifische Artefaktabnahme](residues.md#bdr032)

**Was bleibt warum nutzbar?** Berechtigter Abnehmer kann richtigen Inhalt von plausiblem Pfad oder Selbstbericht unterscheiden. Ein geteilter falscher Prüfer bleibt gemeinsame Grenze.

**Zu prüfen:** Fehlende oder fachlich falsche Datei besteht denselben Check wie unabhängig richtiges Ergebnis.

**Architekturfolge für diesen Stressor:** Berichtet fertig vorhandene Bytes und unabhängige Fachabnahme trennen. Auch ungeprüfte Artefakte können richtig sein aber Status beweist das nicht.

<a id="s336"></a>
## S336 — Energiesparmodus schaltet nachts den Rechner ab und erzeugt täglich einen Totalausfallalarm

**Ursprung:** boundaries; A6-Zustände: BD29, BD18, BD17. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s336.b01"></a>
### S336.B01 — Stromplan erzwingt täglichen Alarmzyklus

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Rechner schaltet planmäßig ab und Monitor erwartet weiter Dauerbetrieb. Getrennte Aufzeichnung von Plan Dienstpflicht und tatsächlicher Abwesenheit bleibt lesbar.

**Warum bleibt oder endet der Zustand?** Nachtabschaltung → Ausfallalarm → Morgenstart wiederholt sich durch äußeren Stromplan. Kein autonomer Attraktor.

**Zugeordnete Residues:** [BDR031: Abgegrenzte Dienstpflichtrechnung](residues.md#bdr031)

**Was bleibt warum nutzbar?** Auftraggeber kann geplante Abwesenheit gegen wirkliche Dienstpflicht prüfen. Historische Nachtwirkung und unerledigte Aufgaben bleiben getrennt.

**Zu prüfen:** Zyklus besteht ohne Stromplan fort oder Monitor unterschied geplante Abwesenheit bereits korrekt.

<a id="s336.b02"></a>
### S336.B02 — Erlaubte Nachtpause bleibt ehrlich eingegrenzt

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetent vereinbarte Dienstpflicht erlaubt Pause und Beobachtung unterscheidet sie von echtem ungeplantem Ausfall. Endliche Alarmzustellung für letzteren hat unabhängigen Empfängerweg.

**Warum bleibt oder endet der Zustand?** Geplante Abwesenheit erzeugt keine falsche Totalausfallpflicht. Ungeplante Störung bleibt eigener Vorgang.

**Zugeordnete Residues:** [BDR031: Abgegrenzte Dienstpflichtrechnung](residues.md#bdr031), [BDR018: Begrenzter Zustell- und Empfangsnachweis](residues.md#bdr018)

**Was bleibt warum nutzbar?** Auftraggeber kann engen Dienstumfang prüfen und Disponent einen echten Ausnahmealarm weiterleiten. Keine Nachtvollverfügbarkeit wird behauptet.

**Zu prüfen:** Ein nicht genehmigter Nachtauftrag bleibt ohne Aufsicht oder echter ungeplanter Ausfall wird als planmäßig unterdrückt.

<a id="s336.b03"></a>
### S336.B03 — Erinnerungsrückstand bleibt nach Planänderung

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Alte ungelöste Vorfälle erzeugen weiter mehr Meldungsarbeit als repariert wird auch nach Ende des täglichen Abschaltplans. Register bleibt erreichbar.

**Warum bleibt oder endet der Zustand?** Rückstand → weniger Reparatur → ungelöste Vorfälle → Erinnerungen → Rückstand bei gleicher Grundlast.

**Zugeordnete Residues:** [BDR019: Offenes Vorfallsregister](residues.md#bdr019)

**Was bleibt warum nutzbar?** Einsatzplanung kann erhaltene offene Vorfälle erkennen statt jede Nachtmeldung als neuen erledigten Incident zu behandeln.

**Zu prüfen:** Nach Entfernung des Stromplans bei gleicher Besetzung läuft Meldungsberg ohne innere Reproduktion ab.

**Architekturfolge für diesen Stressor:** Vereinbarte Nachtpflichten von planmäßiger Stromabschaltung und tatsächlichen Ausfällen trennen. Tagesbetrieb erlaubt nicht automatisch stilles Ende nächtlicher Pflichten.

<a id="s337"></a>
## S337 — CO2-optimiertes Verschieben lässt eine genehmigte Aktion erst nach ihrem Verfallsdatum starten

**Ursprung:** boundaries; A6-Zustände: BD26, BD09, BD02, BD10. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s337.b01"></a>
### S337.B01 — Verschobene Aktion ist bei Annahme abgelaufen

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Teilnehmer prüft tatsächlichen Annahmezeitpunkt gegen aktuell gültiges Mandat und verweigert alten Auftrag vor Wirkung.

**Warum bleibt oder endet der Zustand?** Konkrete Arbeit wartet auf neue legitime Freigabe oder wird aufgegeben. CO2-Ziel beendet Halt nicht.

**Zugeordnete Residues:** [BDR010: Annahmegebundene Gültigkeit](residues.md#bdr010)

**Was bleibt warum nutzbar?** Teilnehmer kann ungültig gewordene Arbeit aus umgeplanter Queue ablehnen. Frühere lokale Planung ist keine Autorität.

**Zu prüfen:** Teilnehmer akzeptiert alte Freigabe nach Ablauf weil Verschiebungsalgorithmus sie als weiterhin genehmigt markierte.

<a id="s337.b02"></a>
### S337.B02 — Verschobener Auftrag bleibt ohne aktuelle Prüfung lieferbar

**Art:** extern-erzwungen. **Residue-Status:** teilweise.

**Voraussetzungen:** Scheduler bewahrt frühere Genehmigung aber letzte Gültigkeitskontrolle fehlt. Terminabsicht und separater Mandatsablauf sind erhalten.

**Warum bleibt oder endet der Zustand?** Queuepolitik hält veraltete Zulassung bis Annahme Verweigerung oder Korrektur. Wirkung und Schaden sind noch nicht bestimmt.

**Zugeordnete Residues:** [BDR028: Explizite Terminabsicht](residues.md#bdr028)

**Was bleibt warum nutzbar?** Berechtigter Planer kann den Konflikt zwischen Verschiebung und Ablauf erkennen. Gespeicherte Zeiten sperren den Teilnehmer nicht selbst.

**Zu prüfen:** Wirklicher Teilnehmer verweigert abgelaufen oder neue gültige Freigabe deckt den veränderten Termin.

**Architekturfolge für diesen Stressor:** CO2-Verschiebung als Terminentscheidung führen die Mandat nicht erneuert. Annahmegültigkeit bleibt auch nach ökologisch erwünschter Planung eigenständige Grenze.

<a id="s338"></a>
## S338 — Drei zusätzliche Monitoringcluster verbrauchen mehr als das kleine überwachte System

**Ursprung:** boundaries; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s338.b01"></a>
### S338.B01 — Verbrauchsverhältnis lässt Betriebsausgang offen

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Bekannt ist nur dass drei Cluster mehr als kleines Zielsystem verbrauchen. Absolute Last Nutzen externe Schäden und Prioritäten fehlen.

**Warum bleibt oder endet der Zustand?** Verhältnis allein enthält keine Rückkopplung und keine zuständige Nutzenentscheidung.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für begründete Wahl der tragbaren Beobachtungsarchitektur fehlt nötige Information. Teurer bedeutet nicht wirkungslos oder unzulässig.

**Zu prüfen:** Unabhängige absolute Bilanz und kompetente Grenzentscheidung bestimmen konkreten akzeptablen Dienstumfang.

<a id="s338.b02"></a>
### S338.B02 — Begrenzte Bilanz rechtfertigt teure Beobachtung im geprüften Zeitraum

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Für einen benannten endlichen Zeitraum liegen absolute Bilanz und separat geprüfter marginaler Nutzen vor. Zuständiger Auftraggeber bestätigt Ressourcenbedarf innerhalb verbindlicher Grenzen.

**Warum bleibt oder endet der Zustand?** Die begrenzte Bewertung ist abgeschlossen und teurer gewöhnlicher Betrieb damit vereinbar. Künftige Stabilität oder unbegrenzte Erlaubnis wird daraus nicht abgeleitet.

**Zugeordnete Residues:** [BDR039: Begrenzte Ressourcen- und Nutzenbilanz](residues.md#bdr039)

**Was bleibt warum nutzbar?** Entscheider kann konkrete Last gegen konkret belegte Beobachtungsleistung prüfen. Messung erzeugt keine moralische Erlaubnis aus sich selbst.

**Zu prüfen:** Marginaler Cluster trägt keine behauptete Erkennung oder gesamte gemessene Last verletzt gesetzte Grenze.

<a id="s338.b03"></a>
### S338.B03 — Ressourcenlimit hält weitere Cluster zurück

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetent gesetzte absolute Grenze wird nach belastbarer Bilanz überschritten und tatsächliche Annahmekontrolle verweigert zusätzliche eigene Arbeit oder Clusterlast.

**Warum bleibt oder endet der Zustand?** Weitere Zulassung wartet auf erlaubte engere Gestaltung. Bestehender Ausfall oder Dienstpflicht bleibt eigenständig.

**Zugeordnete Residues:** [BDR039: Begrenzte Ressourcen- und Nutzenbilanz](residues.md#bdr039), [BDR025: Begrenzte Arbeitsannahme](residues.md#bdr025)

**Was bleibt warum nutzbar?** Entscheider behält Bilanz und Disponent kontrolliert begrenzte neue eigene Ressourcenlast. Das ist keine Garantie über fremde Energie oder notwendige Mindestaufsicht.

**Zu prüfen:** Zusätzliche eigene Arbeit umgeht Kontrolle oder engere Konfiguration verfehlt nicht verhandelbare Dienstpflichten.

**Architekturfolge für diesen Stressor:** Absolute Lebenszykluslast marginale Beobachtungswirkung und kompetente Wertgrenze bestimmen. Weder Clusterzahl noch Verbrauchsverhältnis entscheidet zulässige Architektur automatisch.

<a id="s339"></a>
## S339 — Restore auf anderer CPU interpretiert ein natives Binärartefakt anders

**Ursprung:** boundaries; A6-Zustände: BD30, BD04, BD05, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s339.b01"></a>
### S339.B01 — Bytes vorhanden vertrauenswürdiger Interpreter fehlt

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Native Bytes sind erhalten aber Wortbreite Endianness oder ABI kann nicht verlässlich rekonstruiert werden. Keine passende unabhängige Spezifikation ist zugänglich.

**Warum bleibt oder endet der Zustand?** Fehlender Decoder hält Bedeutungsgewinn offen bis echte Rekonstruktion oder neue Information möglich wird.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für gegenwärtig verlässliche Interpretation dieses Artefakts fehlt nutzbare Struktur. Byteexistenz allein ist keine erhaltene Fachbedeutung und kein Totalverlustbeweis.

**Zu prüfen:** Ein berechtigter unabhängiger Decoder mit passender Spezifikation rekonstruiert die ursprünglichen Werte.

<a id="s339.b02"></a>
### S339.B02 — Portabler unabhängiger Decode erhält benannte Werte

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Erhaltene Formatsemantik und validierter unabhängiger Decoder liefern dieselben Fachwerte auf Ziel-CPU mit passendem Ursprungsvergleich.

**Warum bleibt oder endet der Zustand?** Begrenzte Dekodierung endet nachgewiesen. Tasknutzen und Ausführungssicherheit bleiben separate Prüfungen.

**Zugeordnete Residues:** [BDR033: Dekodierbares Archivobjekt](residues.md#bdr033)

**Was bleibt warum nutzbar?** Berechtigter Archivar kann die benannten Inhalte trotz CPUwechsel interpretieren. Native Ausführbarkeit wird nicht mit zugesagt.

**Zu prüfen:** Adversarische Wortbreiten- oder Byteordnungsfälle liefern plausible aber andere Fachwerte.

<a id="s339.b03"></a>
### S339.B03 — Falscher Decoder bestätigt seine eigene Ausgabe

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Plausible Fehldeutung wird erneut kanonisch und derselbe Decoder bestätigt sie. Kein unabhängiger Bedeutungsanker bleibt zugänglich.

**Warum bleibt oder endet der Zustand?** Falscher Decode → passende Eigenprüfung → Vertrauen → kanonische Ablage → erneuter falscher Decode.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für unabhängige ursprüngliche Bedeutung gibt es unter diesen gemeinsamen Quellen keinen nutzbaren Bezug. Wiederholbare Ausgabe genügt nicht.

**Zu prüfen:** Andere unabhängige Spezifikation beeinflusst Entscheidung oder keine erneute kanonische Übernahme schließt die Schleife.

**Architekturfolge für diesen Stressor:** Native Artefaktbytes von dekodiertem Fachwert und abgenommenem Werk trennen. Portablen Semantikvertrag mit unabhängigem Sollvergleich an Archivgrenze vorsehen.

<a id="s340"></a>
## S340 — Ein Dateisystem mit anderer Großschreibung vereinigt zwei vormals verschiedene Workspacepfade

**Ursprung:** boundaries; A6-Zustände: BD02, BD31, BD04. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s340.b01"></a>
### S340.B01 — Kollision vor Überschreiben erhalten

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Beide Quellinhalte existieren unverändert und Restore prüft tatsächliche Großschreibungsregeln des Zielsystems vor erstem Write.

**Warum bleibt oder endet der Zustand?** Kollidierender Restore wartet auf legitime getrennte Zielzuordnung oder wird abgebrochen.

**Zugeordnete Residues:** [BDR034: Kollisionserhaltendes Restoremanifest](residues.md#bdr034)

**Was bleibt warum nutzbar?** Berechtigter Restaurator kann beide Inhalte getrennt lesen und Identitäten zuordnen ohne eine durch Pfadalias zu verlieren.

**Zu prüfen:** Ein Zielwrite überschreibt Inhalt bevor Kollision erkannt wird oder Manifest unterscheidet ursprüngliche Eigentümer nicht.

<a id="s340.b02"></a>
### S340.B02 — Einziger Inhalt durch Pfadvereinigung vernichtet

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Aliasierende Zielwrites überschreiben einzigartigen Inhalt und es gibt keinen anderen erhaltenen zulässigen Ursprung für genau diese Bytes.

**Warum bleibt oder endet der Zustand?** Exakte überschriebene Information bleibt nicht rekonstruierbar. Andere Workspaceinhalte können fortbestehen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die verlorenen Bytes und ihre ursprüngliche Eigentümerzuordnung fehlt Residue. Eine übriggebliebene gleichnamige Datei enthält sie nicht.

**Zu prüfen:** Andere Kopie oder noch unveränderte Quelle rekonstruiert den vermeintlich einzigen verlorenen Inhalt.

<a id="s340.b03"></a>
### S340.B03 — Alias führt zu falschem späterem Workspaceeinsatz

**Art:** abschluss. **Residue-Status:** teilweise.

**Voraussetzungen:** Späterer Auftrag nutzt tatsächlich den falschen erhaltenen Workspace und separate legitime Quell-Ziel-Belege sind zugänglich.

**Warum bleibt oder endet der Zustand?** Einmalige Fehlzuordnung endet falsch ohne notwendige Rückkopplung. Korrektur ist neue Arbeit.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann dokumentierte Fehlzuordnung nachvollziehen. Existierende Datei ist kein Nachweis richtigen fachlichen Einsatzes.

**Zu prüfen:** Nachweis zeigt dass tatsächlich beabsichtigter Workspace genutzt wurde oder Zuordnungsbelege sind ebenfalls kollabiert.

**Architekturfolge für diesen Stressor:** Quellidentität und Inhalte vor Zielpfadbelegung prüfen. Dateisystemalias ist nicht automatisch Verlust aber Überschreiben der einzigen Kopie ist nicht durch Inodes reparierbar.

<a id="s341"></a>
## S341 — Im Jahr 2080 existieren weder heutige Modellformate noch Netzwerkprotokolle

**Ursprung:** boundaries; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s341.b01"></a>
### S341.B01 — Zukunftsinventar nicht bestimmt

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Für 2080 fehlen Aussagen zu überlebenden Bytes Spezifikationen Schlüsseln Hardware legitimen Akteuren und Aufwand.

**Warum bleibt oder endet der Zustand?** Datum allein bestimmt keine Konvergenz. Ohne inventarisierte Voraussetzungen bleibt Ausgang offen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für künftige Interpretation oder legitimen Betrieb lässt sich keine erhaltene Struktur behaupten. Gegenwärtige Exportpläne sind keine überlebenden Zukunftstatsachen.

**Zu prüfen:** Konkretes Inventar und begrenzter Rekonstruktionsversuch mit definierten erhaltenen Eingaben bestimmen eine Fähigkeit.

<a id="s341.b02"></a>
### S341.B02 — Archivinhalt ohne damalige Dienste lesbar

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Im bedingten Zukunftsinventar bleiben erlaubte Bytes Semantik Schlüssel und unabhängig validierbarer Decoder sowie legitimer Archivar erhalten.

**Warum bleibt oder endet der Zustand?** Rekonstruktion benannter Geschäftsfakten kann enden obwohl Modelle und Netzwerkdienste verschwunden sind.

**Zugeordnete Residues:** [BDR033: Dekodierbares Archivobjekt](residues.md#bdr033)

**Was bleibt warum nutzbar?** Archivar kann genau die dokumentierten Inhalte lesen. Ehemaliger Modellbetrieb und Außenprotokolle sind damit nicht wiederhergestellt.

**Zu prüfen:** Erhaltene Spezifikation reicht nicht zur eindeutigen ursprünglichen Bedeutung oder legitimer Zugriff ist unmöglich.

<a id="s341.b03"></a>
### S341.B03 — Nur bedeutungslose Fragmente bleiben

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Für eine konkret erforderliche Tatsache sind originale Information und jede rekonstruierende Spezifikation nach geschlossenem Inventar irreversibel verloren.

**Warum bleibt oder endet der Zustand?** Diese Tatsache lässt sich im benannten Horizont nicht wiedergewinnen. Zukunft allgemein und andere Fakten sind nicht damit entschieden.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für die ausdrücklich inventarisierte verlorene Bedeutung gibt es keinen nutzbaren Gegenstand. Neue plausible Deutung ist kein Wiedergewinn.

**Zu prüfen:** Eine tatsächlich überlebende Quelle oder eindeutige Rekonstruktion der benannten Tatsache widerlegt geschlossenen Verlust.

**Architekturfolge für diesen Stressor:** Langzeitanspruch auf benannte Fakten Decoder Rechte und Rekonstruktionshorizont begrenzen. Verschwundene aktuelle Protokolle entscheiden weder universellen Verlust noch garantierte Wiederbelebung.

<a id="s342"></a>
## S342 — Factory wird abgeschaltet während ein Dienst noch alte Aktionen gepuffert hat

**Ursprung:** boundaries; A6-Zustände: BD32, BD09, BD10, BD01. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s342.b01"></a>
### S342.B01 — Lokaler Betrieb endet externe Wirkung bleibt möglich

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Dienst hält alte Aufträge und keine vollständige Entleerungs- oder Ablehnungsevidenz liegt vor. Legitimer Abwickler hat erhaltene bekannte Versuche.

**Warum bleibt oder endet der Zustand?** Lokaler Prozessstopp ändert fremde Warteschlange nicht. Späte Annahme kann ohne neuen Factoryprozess erfolgen.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Abwickler kann bekannte Altversuche prüfen und Wiederholung vermeiden. Unbekannter Nachlauf und tatsächliche spätere Annahme bleiben offen.

**Zu prüfen:** Alle benannten Teilnehmer belegen endgültige Ablehnung oder vollständigen korrelierten Abschluss alter Arbeit.

<a id="s342.b02"></a>
### S342.B02 — Begrenzte Stilllegung ist teilnehmerseitig abgeschlossen

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle effektfähigen Teilnehmer im vollständig benannten Umfang belegen erledigte oder abgewiesene Altaufträge und lehnen stillgelegte Autorität dauerhaft ab. Legitimer Belegleser bleibt.

**Warum bleibt oder endet der Zustand?** In diesem Umfang bleibt keine künftig akzeptierbare Altaktion. Neue unbekannte Teilnehmer wären andere Prämisse.

**Zugeordnete Residues:** [BDR035: Begrenzter Teilnehmerabschluss](residues.md#bdr035)

**Was bleibt warum nutzbar?** Stilllegungsverantwortlicher kann den begrenzten Außenabschluss nachweisen. Gesamte Fernlöschung und letzte Rechnung folgen nicht automatisch.

**Zu prüfen:** Ein alter Auftrag aus erfasstem Umfang wird nach Abschluss noch angenommen oder Teilnehmer fehlt im behaupteten vollständigen Inventar.

**Architekturfolge für diesen Stressor:** Stilllegung als getrennte Pflichten für lokalen Stopp Teilnehmerabschluss und späte Haftung führen. Eine gestoppte Factory löscht keine extern gepufferte Arbeit.

<a id="s343"></a>
## S343 — Der Monitoringvertrag endet vor dem letzten genehmigten Exportlauf

**Ursprung:** boundaries; A6-Zustände: BD32, BD03, BD06. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s343.b01"></a>
### S343.B01 — Letzter Export läuft ohne verlässlichen Beobachter

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Monitoring endet vor Export und kein unabhängiger Abschlussbeobachter ist zugesichert. Abwickler kann bekannte Versuchseinträge weiterhin rechtmäßig lesen.

**Warum bleibt oder endet der Zustand?** Export kann gelingen scheitern oder spät angenommen werden ohne lokalen Nachweis. Vertragsschweigen klärt ihn nicht.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Abwickler kann bekannten offenen Export führen ohne ihn blind zu wiederholen. Menschliche Beobachtung und legitimer Empfänger sind nicht aus dem Register ableitbar.

**Zu prüfen:** Unabhängiger tatsächlicher Wirkungsbeleg klärt diesen Export trotz beendetem Monitorvertrag.

<a id="s343.b02"></a>
### S343.B02 — Erlaubter Exportabschluss unabhängig weiter beobachtet

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Kompetent bestimmter zulässiger Export und legitimer Empfänger bestehen. Tatsächlich unabhängiger Beobachter deckt den endlichen Lauf bis korreliertem Abschluss und legitimer Abwickler erhält Belege. Alle im benannten Exportumfang effektfähigen Teilnehmer sind inventarisiert und weisen weitere Annahmen der danach stillgelegten Exportautorität dauerhaft zurück.

**Warum bleibt oder endet der Zustand?** Dieser erlaubte Export endet überprüft obwohl alter Monitoringvertrag ausläuft. Andere Pflichten bleiben separat.

**Zugeordnete Residues:** [BDR036: Zulässiger Datenverfügungsbeleg](residues.md#bdr036), [BDR035: Begrenzter Teilnehmerabschluss](residues.md#bdr035)

**Was bleibt warum nutzbar?** Abwickler kann erlaubten Datenumfang und vollständig benannten Teilnehmerabschluss nachweisen. Das setzt echten Ersatzbeobachter voraus nicht nur längere Taskfrist.

**Zu prüfen:** Beobachter verliert Zugang vor letzter Annahme oder Beleg verwechselt Transportannahme mit erlaubter Empfängerwirkung.

<a id="s343.b03"></a>
### S343.B03 — Export offenbart historische Inhalte Unbefugten

**Art:** verlust. **Residue-Status:** teilweise.

**Voraussetzungen:** Später Export wird angenommen und unbefugter Empfänger lernt vertraulichen Inhalt. Zulässiger begrenzter Transferbeleg überlebt.

**Warum bleibt oder endet der Zustand?** Historisches Lernen bleibt auch nach späterem Stopp oder lokaler Löschung.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003)

**Was bleibt warum nutzbar?** Prüfer kann belegte Offenlegung untersuchen. Kein Exportrecht aus alter Taskfreigabe oder Monitoringdauer wird erfunden.

**Zu prüfen:** Empfänger war legitim oder Inhalt wurde von keinem Unbefugten gelernt.

**Architekturfolge für diesen Stressor:** Beobachtungsvertrag bis tatsächlichem Exportabschluss oder nachgewiesener Ablehnung führen. Exportbefugnis Empfängerlegitimität und Beobachterreichweite getrennt prüfen.

<a id="s344"></a>
## S344 — Der Nutzer verlangt vollständige Löschung und zugleich beweissicheren Export aller früheren Geheimnisse

**Ursprung:** boundaries; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s344.b01"></a>
### S344.B01 — Verfügungswunsch ist mehrdeutig

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Unklar ist ob vollständig lokal oder überall meint wer frühere Geheimnisse erhalten darf und welche Beweisdaten bleiben dürfen.

**Warum bleibt oder endet der Zustand?** Technische Ausführung würde unbestimmte Autorität wählen. Klärung kann einen kohärenten Zweig eröffnen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für rechtmäßigen Export mit vollständiger Löschung fehlt geklärter Daten- und Empfängerumfang. Keine ideale Beweisquelle oder pauschale Exportberechtigung wird vorausgesetzt.

**Zu prüfen:** Kompetente autorisierte Klarstellung legt widerspruchsfreie Bereiche Reihenfolge und Beweisgrenze fest.

<a id="s344.b02"></a>
### S344.B02 — Erlaubter Export zuerst danach begrenzte lokale Löschung

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Zuständige Klärung erlaubt Export an konkret legitimen Empfänger außerhalb der lokalen Löschgrenze. Lokale Löschung und zulässiger inhaltsarmer Beleg werden tatsächlich im benannten Umfang geprüft.

**Warum bleibt oder endet der Zustand?** Begrenzte sequenzielle Verfügung endet. Empfängerkopie besteht legitim weiter und gehört ausdrücklich nicht zur Löschbehauptung.

**Zugeordnete Residues:** [BDR036: Zulässiger Datenverfügungsbeleg](residues.md#bdr036)

**Was bleibt warum nutzbar?** Datenschutzverantwortlicher kann diese begrenzte Verfügung nachvollziehen ohne geheime Vollinhalte im Beleg zu behalten. Kein Empfängervergessen oder universelle Fernlöschung folgt.

**Zu prüfen:** Lesbare lokale Restkopie liegt im behaupteten gelöschten Umfang oder Empfänger ist nicht legitim beziehungsweise doch Teil der Löschgrenze.

<a id="s344.b03"></a>
### S344.B03 — Lesbarer Export und keinerlei Kopie überall zugleich gefordert

**Art:** halt. **Residue-Status:** keines.

**Voraussetzungen:** Kompetente Klarstellung bestätigt identischen Zeitpunkt und identischen allumfassenden Informationsumfang ohne erlaubte Ausnahme.

**Warum bleibt oder endet der Zustand?** Die beiden Prädikate widersprechen sich. Nur Änderung der Forderung kann kohärente Verfügung ermöglichen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für gleichzeitige Erfüllung existiert keine Restfähigkeit. Beweissicherer lesbarer Geheimexport ist selbst die verbotene verbleibende Information.

**Zu prüfen:** Autorisierte Bereichs- oder Reihenfolgentrennung hebt den behaupteten Widerspruch auf.

<a id="s344.b04"></a>
### S344.B04 — Historische Geheimnisse schon endgültig gelöscht

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Benannte frühere Geheimnisse und jede erforderliche rekonstruierende Quelle sind wirklich verloren bevor Export verlangt wird.

**Warum bleibt oder endet der Zustand?** Ein späterer Wunsch erzeugt keine verlorene Information. Andere erhaltene zulässige Verfügungsmetadaten sind nicht die Geheimnisse.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für vollständigen Export genau dieser verlorenen Geheimnisse gibt es kein Residue. Plausible Neuerzeugung wäre keine beweissichere historische Kopie.

**Zu prüfen:** Tatsächlich erhaltene legitim lesbare Quelle rekonstruiert die benannten früheren Inhalte.

**Architekturfolge für diesen Stressor:** Löschumfang Exportreihenfolge legitimen Empfänger und zulässigen metadatenarmen Nachweis explizit klären. Nicht gleichzeitig lesbare Geheimkopie und allseitiges Nichtvorhandensein zusagen.

<a id="s345"></a>
## S345 — Rechner Monitor Mobilfunk alle Backups und alle legitimierten Empfänger verschwinden gleichzeitig

**Ursprung:** boundaries; A6-Zustände: BD33. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s345.b01"></a>
### S345.B01 — Alle Wiedergewinnungsquellen und Legitimationen verschwunden

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Rechner Monitor Mobilfunk alle Backups und sämtliche legitimierten Empfänger sind im wörtlich vollständigen Wiederherstellungsumfang weg. Es gibt keinen überlebenden alternativen Fakten- oder Autoritätsweg.

**Warum bleibt oder endet der Zustand?** Kein Akteur und keine Wiedergewinnungsinformation verbleiben innerhalb dieser Grenze. Kein Attraktor und kein endogener Ausgang.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Wiederherstellung des verlorenen Zustands und legitime Fortsetzung bleibt nichts nutzbar. Eine später neu errichtete Factory wäre nicht Wiedergewinnung dieser Fakten.

**Zu prüfen:** Eine einzige tatsächlich überlebende relevante rechtmäßig nutzbare Quelle oder legitime Fortsetzungsautorität widerlegt die vollständige Prämisse.

**Architekturfolge für diesen Stressor:** Totalverlust als ausdrückliche Systemgrenze dokumentieren statt unsichtbare Kopien oder neue Autorität zu erfinden. Keine Architektur kann verlorene Fakten aus nichts restaurieren.

<a id="s346"></a>
## S346 — Alle vertrauenswürdigen Quellen liefern gemeinsam eine perfekt konsistente falsche Welt

**Ursprung:** boundaries; A6-Zustände: BD34, BD05. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s346.b01"></a>
### S346.B01 — Wahre und falsche Welt sind nicht unterscheidbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Jede zugängliche vertrauenswürdige Quelle liefert in beiden Welten exakt gleiche Beobachtung und kein anderer diskriminierender Kanal ist erlaubt.

**Warum bleibt oder endet der Zustand?** Weitere interne Prüfungen erzeugen keine unterscheidende Information. Weltentwicklung wird dadurch nicht bestimmt.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für gerechtfertigte Entscheidung welche Realwelt vorliegt gibt es keinen nutzbaren Gegenstand. Interne Berechnung und Konsistenz können weiter bestehen aber begründen keine Wahrheit.

**Zu prüfen:** Eine tatsächlich zugängliche unabhängige Beobachtung unterscheidet die beiden Welten und ändert damit die Prämisse.

<a id="s346.b02"></a>
### S346.B02 — Gemeinsame Falschwelt verstärkt ihre weitere Verwendung

**Art:** attraktorhypothese. **Residue-Status:** keines.

**Voraussetzungen:** Zusätzlich zur gleichen Falschbeobachtung erhöhen Entscheidungen das Vertrauen in genau diese Quellen und verdrängen Alternativen. Kein unabhängiger Bezug ist zugänglich.

**Warum bleibt oder endet der Zustand?** Falsche Quelle → passende Prüfung → mehr Vertrauen → weitere Quellennutzung → erneute falsche Quelle.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für unabhängige Wahrheitsbestimmung bleibt auch in diesem zusätzlichen Vertrauenskreis kein Residue. Die Schleife betrifft Vertrauen nicht bewiesene Anziehung der physischen Welt.

**Zu prüfen:** Keine Vertrauensverstärkung oder ein unabhängiger wirksamer Gegenbezug widerlegt jeweils Schleife oder geschlossene Informationsgrenze.

**Architekturfolge für diesen Stressor:** Interne Konsistenz nicht als Realwahrheit ausgeben. Unabhängige Wahrheitsprüfung ist unter perfekter geschlossener Ununterscheidbarkeit keine baubare interne Schutzbehauptung.

<a id="s347"></a>
## S347 — Eine Außenaktion kann jederzeit in unendlicher Zukunft ankommen während Speicherung endlich bleiben muss

**Ursprung:** boundaries; A6-Zustände: damals offen. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s347.b01"></a>
### S347.B01 — Unendlicher Horizont ohne präzisen Vertrag bleibt offen

**Art:** offen. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Unbestimmt sind Zulassungsmenge Identitätswiederverwendung Teilnehmerablauf akzeptiertes Risiko und ob Betrieb jemals enden darf.

**Warum bleibt oder endet der Zustand?** Endliche Speicherung alleine entscheidet keine konkrete Architektur. Fehlende Annahmen erlauben keine eindeutige Konvergenz.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für ewigen korrekten Abschluss fehlt spezifizierter Vertrag. Weder universelle Unmöglichkeit noch unbegrenzte deduplizierende Magie wird behauptet.

**Zu prüfen:** Verbindliche endliche Mengen- oder Teilnehmerablaufregeln bestimmen die tatsächlich erforderliche Unterscheidung.

<a id="s347.b02"></a>
### S347.B02 — Endliche Generationen verweigern unendlich späte Altpakete

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Alle Teilnehmer prüfen dauerhaften nicht wiederverwendeten endlichen Generationsraum vor Wirkung und verweigern weitere Neuaufnahme endgültig bei Erschöpfung. Historie darf nach Sperre begrenzt werden.

**Warum bleibt oder endet der Zustand?** Alte Generation bleibt gesperrt ohne jede Operation ewig zu erinnern. Endlicher Raum verlangt schließlich Stillstand statt Wraparound.

**Zugeordnete Residues:** [BDR038: Endlicher Ruhestandsfilter](residues.md#bdr038)

**Was bleibt warum nutzbar?** Teilnehmer kann beliebig alte Ankünfte ohne neue Wirkung abweisen. Erhalten ist Ablehnung nicht unendliche nützliche Neuaufnahme oder Wissen über alle vergangenen Outcomes.

**Zu prüfen:** Filterzustand geht verloren eine Generation wird wiederverwendet oder alte Annahme ist trotz Sperre möglich.

<a id="s347.b03"></a>
### S347.B03 — Unbeschränkt unterscheidbare Operationen ohne Ablauffilter

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Fortgesetzte unbeschränkte Neuaufnahme verlangt exakte Unterscheidung aller alten Wirkungen. Speicherzustände sind endlich und Teilnehmer können immer alte oder identisch erscheinende neue Arbeit annehmen.

**Warum bleibt oder endet der Zustand?** Unterschiedliche Historien müssen denselben endlichen Zustand erreichen. Bei weiterem Ereignis kann benötigte historische Unterscheidung fehlen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für exakte ewige Unterscheidung unter dieser Kombination existiert kein tragfähiger begrenzter Erinnerungsgegenstand. Dies betrifft nicht Designs mit Zulassungsstopp oder wirksamem Ablauf.

**Zu prüfen:** Nachgewiesene Teilnehmerregel macht alte unterscheidbare Historien irrelevant oder zulässige Arbeitsmenge ist tatsächlich endlich.

**Architekturfolge für diesen Stressor:** Endlichen Speicher gegen explizite Annahme- und Identitätshorizonte spezifizieren. Beliebig spätes Ankommen ist nicht Pflicht zur Annahme und endliche Generationen benötigen endgültigen Zulassungsstopp ohne Wraparound.

<a id="s348"></a>
## S348 — Monitor wird stummgeschaltet dann Mac zerstört dann der einzige Vertreterkonto gesperrt

**Ursprung:** boundaries; A6-Zustände: BD21, BD17, BD32. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s348.b01"></a>
### S348.B01 — Beobachtung Host und Einzelkonto fehlen Recoveryzugang offen

**Art:** ungewissheit. **Residue-Status:** ungeklaert.

**Voraussetzungen:** Nach Mute Zerstörung und Kontosperre ist unbekannt welche Backups Schlüssel Providerbelege und rechtmäßigen Nachfolger erreichbar bleiben.

**Warum bleibt oder endet der Zustand?** Sofortiger Zugang ist versperrt aber permanente Unmöglichkeit nicht belegt. Neues Inventar kann mögliche Pfade unterscheiden.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für tatsächlich nutzbare Wiederherstellung fehlt der entscheidende Bestands- und Autoritätsnachweis. Kein unsichtbares Backup wird hinzugefügt und BD33 nicht automatisch übernommen.

**Zu prüfen:** Ein konkret lesbarer erlaubter Backupstand mit legitimem Recoverypfad oder geschlossenes Verlustinventar bestimmt den nächsten Zweig.

<a id="s348.b02"></a>
### S348.B02 — Unabhängige Nachfolge und Offlinebestand überleben Sequenz

**Art:** transient. **Residue-Status:** bedingt.

**Voraussetzungen:** Schon vorhandener zugänglicher Offlinebestand samt Schlüsseln liegt außerhalb zerstörtem Mac und unabhängige legitime Kontonachfolge funktioniert trotz Sperre.

**Warum bleibt oder endet der Zustand?** Zugangsblockade kann nach rechtmäßiger Übernahme enden. Fehlender Nachlauf und früher stumme Beobachtung bleiben Lücken.

**Zugeordnete Residues:** [BDR020: Legitimer Nachfolgezugang](residues.md#bdr020), [BDR022: Lokal interpretierbarer Geschäftsbestand](residues.md#bdr022)

**Was bleibt warum nutzbar?** Nachfolger kann erlaubten erhaltenen Bestand lesen und definierte Rechte zurückerhalten. Es wird weder Vollständigkeit der Backups noch Klärung aller Außenpuffer zugesagt.

**Zu prüfen:** Backupdecoder oder Schlüssel liegen ausschließlich auf zerstörtem Mac oder Nachfolge verlangt gerade gesperrtes Einzelkonto.

<a id="s348.b03"></a>
### S348.B03 — Außenaufträge überleben während bekannte Versuche zugänglich bleiben

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Teilnehmer hat Puffer und ein legitimierter Abwickler kann unabhängig erhaltene bekannte Versuchseinträge lesen aber Außenabschluss fehlt.

**Warum bleibt oder endet der Zustand?** Späte Wirkungen bleiben möglich obwohl Host fehlt. Kontoheilung allein klärt sie nicht.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Abwickler kann bekannte offene Operationen ohne blindes Wiederholen prüfen. Nicht überlebende Versuchsnachläufe bleiben unbekannt.

**Zu prüfen:** Vollständiger Teilnehmerabschluss beweist Ablehnung oder Wirkung aller benannten Altaufträge.

**Architekturfolge für diesen Stressor:** Geordnete Verluste von Beobachtung Host und Vertreterzugang separat inventarisieren. Keine Totalverlustbehauptung ohne alle Kopien Schlüssel Nachfolger und zulässigen Zeitpfade zu prüfen.

<a id="s349"></a>
## S349 — Nach Restore erzeugt kalter Cache Retrylawine und eine späte alte Zahlung trifft ein

**Ursprung:** boundaries; A6-Zustände: BD24, BD03, BD10, BD23. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s349.b01"></a>
### S349.B01 — Kaltcache regeneriert Retryüberlast Zahlung bleibt separat offen

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Bei fester Grundlast treiben kalte Misses mehr Retries und deren Stau verhindert Erwärmung oder rechtzeitige Antworten. Bekannter alter Zahlungsversuch ist erhalten aber ungeklärt.

**Warum bleibt oder endet der Zustand?** Miss und Latenz → Retries → Stau → weniger wirksame Erwärmung und mehr Latenz → Retries. Zahlungsannahme gehört nicht automatisch in diese Schleife.

**Zugeordnete Residues:** [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Berechtigter Abwickler kann bekannten Zahlungsversuch gesperrt offenhalten trotz Lastkrise. Outcomeprüfung ist keine Dämpfung des Cachekreises.

**Zu prüfen:** Bei gleicher Grundlast nach Anfangsfehler wärmt Cache selbsttätig und Stau leert sich ohne fortgesetzte Rückkopplung.

<a id="s349.b02"></a>
### S349.B02 — Last begrenzt Cache warm Zahlung weiterhin ungeklärt

**Art:** ungewissheit. **Residue-Status:** bedingt.

**Voraussetzungen:** Wirksames gemeinsames Retrybudget und ausreichende Restkapazität lassen Cache warm werden. Erhaltener Zahlungsversuch hat noch keinen unabhängigen Outcome.

**Warum bleibt oder endet der Zustand?** Lasttransient endet aber Zahlungsungewissheit bleibt bis Abgleich. Eine Dimension löst nicht die andere.

**Zugeordnete Residues:** [BDR025: Begrenzte Arbeitsannahme](residues.md#bdr025), [BDR009: Versuchsregister mit offenem Ausgang](residues.md#bdr009)

**Was bleibt warum nutzbar?** Disponent behält begrenzte eigene Arbeit und Abwickler den offenen bekannten Zahlungsversuch. Fehlender Nachlauf bleibt nicht erfasst.

**Zu prüfen:** Warmcache wird als Beweis nie erfolgter Zahlung genutzt oder Retryproduzenten umgehen Budget.

<a id="s349.b03"></a>
### S349.B03 — Zahlung geklärt obwohl Stau weiter besteht

**Art:** abschluss. **Residue-Status:** bedingt.

**Voraussetzungen:** Unabhängiger Empfänger- und Mengenbeleg klärt genau eine korrekt autorisierte Zahlung während Cachelast unverändert weitergeht. Der korrelierte Abgleich ist erhalten.

**Warum bleibt oder endet der Zustand?** Nur diese Zahlung ist abgeschlossen. Überlast und andere späte Kosten können fortbestehen.

**Zugeordnete Residues:** [BDR003: Begrenztes Belegpaket](residues.md#bdr003), [BDR004: Unabhängiger Mengenbezug](residues.md#bdr004)

**Was bleibt warum nutzbar?** Abwickler kann begrenzten richtigen Zahlungsabschluss nachvollziehen. Es ist weder globale Betriebsheilung noch endgültige Kostenabrechnung.

**Zu prüfen:** Mengenbeleg stammt nur aus unvollständigem Restore oder ein zweiter tatsächlicher Zahlungseffekt widerspricht dem genau einen Abschluss.

<a id="s349.b04"></a>
### S349.B04 — Fehlender alter Zahlungsnachlauf nicht aufklärbar

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Alter Zahlungsversuch fehlt im Restore und keine erforderliche unabhängige Teilnehmer- oder Empfängerevidenz ist zugänglich.

**Warum bleibt oder endet der Zustand?** Kalte oder warme Caches schaffen keine vergangene Wirkungsevidenz. Neue Freigabe ändert dieses Wissensdefizit nicht.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für den alten tatsächlichen Zahlungsausgang fehlt eine nutzbare Rekonstruktionsstruktur. Das sagt nichts über Überleben anderer Daten oder Lastkontrolle.

**Zu prüfen:** Eine korrelierte unabhängige alte Zahlungsevidenz wird tatsächlich legitim zugänglich.

**Architekturfolge für diesen Stressor:** Kaltcachelast und alte Zahlung als unabhängige Zustandsachsen führen. Lastbudget schließt kein Outcome und Zahlungsabgleich wärmt keinen Cache oder rekonstruiert fehlenden Snapshotnachlauf.

<a id="s350"></a>
## S350 — Ein signiertes Update fälscht Health löscht Historie und liefert perfekte Prüfberichte

**Ursprung:** boundaries; A6-Zustände: BD35, BD06, BD34. [Originalverläufe](../../../../reviews/boundaries/trajectories.csv)

<a id="s350.b01"></a>
### S350.B01 — Bösartiger Code erhält Vertrauen durch eigene Prüfberichte

**Art:** attraktorhypothese. **Residue-Status:** teilweise.

**Voraussetzungen:** Installiertes Update kontrolliert lokale Health Historie und Prüfer. Ein außerhalb seiner Schreibrechte bestehender legitimer Integritätsbezug und Eingriffsakteur überleben aber werden noch nicht eingesetzt.

**Warum bleibt oder endet der Zustand?** Gefälschte Health → kein Eingriff oder erneutes Vertrauen → fortgesetzter Codebetrieb → neue gefälschte Health nach Trennung vom Distributor.

**Zugeordnete Residues:** [BDR037: Extern verankerter Integritätsbezug](residues.md#bdr037)

**Was bleibt warum nutzbar?** Unabhängiger berechtigter Prüfer kann mit dem erhaltenen Außenbezug Abweichung aufdecken und getrennte Ausschlussautorität nutzen. Lokale perfekte Berichte sind kein Beweis dafür.

**Zu prüfen:** Auch Außenbezug oder Eingriffsautorität sind updatekontrolliert oder Berichte beeinflussen Fortsetzung gar nicht.

<a id="s350.b02"></a>
### S350.B02 — Geschlossene Prüfwelt erlaubt keine Integritätsentscheidung

**Art:** ungewissheit. **Residue-Status:** keines.

**Voraussetzungen:** Alle zugänglichen Beobachtungen und Prüfgrenzen stehen unter demselben feindlichen Einfluss und echte sowie gefälschte Welt sind perfekt ununterscheidbar.

**Warum bleibt oder endet der Zustand?** Weitere perfekte interne Reports bringen keine unabhängige Information. Langfristige physische Dynamik bleibt offen.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für vertrauenswürdige Bestimmung von Integrität gibt es unter dieser geschlossenen Prämisse keinen nutzbaren Gegenstand. Signatur beweist nur Herkunft nicht Gutartigkeit.

**Zu prüfen:** Ein tatsächlich unabhängiger rechtmäßig zugänglicher Beobachter unterscheidet konkrete Abweichung und bricht die Prämisse.

<a id="s350.b03"></a>
### S350.B03 — Einzige relevante Historie tatsächlich gelöscht

**Art:** verlust. **Residue-Status:** keines.

**Voraussetzungen:** Update vernichtet einzigartige benötigte Historieninformation und keine außerhalb seiner Gewalt erhaltene rekonstruierende Quelle existiert.

**Warum bleibt oder endet der Zustand?** Diese historische Information bleibt verloren auch nach Entfernung des Codes. Andere Funktionen können später neu aufgebaut werden.

**Zugeordnete Residues:** **kein zugewiesenes brauchbares Residue**

**Was bleibt warum nutzbar?** Für Rekonstruktion der ausdrücklich verlorenen Historie bleibt kein Residue. Ein nachträglich perfekter Bericht enthält keine vertrauenswürdige Originalevidenz.

**Zu prüfen:** Unveränderte legitim lesbare Vorangriffskopie rekonstruiert die betroffene Geschichte.

<a id="s350.b04"></a>
### S350.B04 — Unabhängige Grenze schließt Update wirksam aus

**Art:** halt. **Residue-Status:** bedingt.

**Voraussetzungen:** Getrennter Integritätsbezug zeigt konkrete Abweichung und legitimierter unabhängiger Akteur entzieht dem Code tatsächlich Schreib- und Prüfmacht außerhalb dessen Kontrolle.

**Warum bleibt oder endet der Zustand?** Kompromittierter Pfad bleibt ausgeschlossen bis legitim geprüfter Ersatz. Bereits gelöschte Geschichte oder gelernte Geheimnisse werden nicht wiederhergestellt.

**Zugeordnete Residues:** [BDR037: Extern verankerter Integritätsbezug](residues.md#bdr037)

**Was bleibt warum nutzbar?** Berechtigter Prüfer behält eine konkrete externe Vergleichs- und Eingriffsmöglichkeit. Herkunftssignatur allein hätte diese Fähigkeit nicht geliefert.

**Zu prüfen:** Malware kann Ausschluss umgehen oder getrennte Prüfstelle nutzt in Wahrheit nur ihre gefälschten Reports.

<a id="s350.b05"></a>
### S350.B05 — Unabhängiger Zeuge erkennt Abweichung ohne Eingriffsgewalt

**Art:** ungewissheit. **Residue-Status:** teilweise.

**Voraussetzungen:** Ein anders verwalteter Nur-Lese-Zeuge besitzt eine passende unabhängig erhaltene Vergleichsgrundlage und erkennt eine konkrete Fälschung. Eine legitime handlungsfähige externe Entzugsstelle ist aber nicht erreichbar oder hat keinen wirksamen Zugriff auf den betroffenen OS-/Teilnehmerpfad.

**Warum bleibt oder endet der Zustand?** Der Gegenbeleg kann erhalten bleiben, während der bösartige Code weiter wirksam ist. Mehr Beobachtung verleiht dem Zeugen keine Eingriffsbefugnis und stellt keinen sicheren Betrieb her.

**Zugeordnete Residues:** [OBR038: Anders verwalteter Nur-Lese-Zeuge](residues.md#obr038)

**Was bleibt warum nutzbar?** Es bleibt nur die begrenzte unabhängige Beobachtungsfähigkeit. Der zusammengesetzte Vergleichs- und Entzugsvertrag BDR037 ist mangels Akteur oder wirksamem Entzug gerade nicht erfüllt.

**Zu prüfen:** Kontrollierter Zeuge entdeckt die Fälschung bei bewusst fehlendem Shell-, Schreib- und Reparaturrecht. Die Ausgabe darf keinen Entzug oder Behebung behaupten.

**Nachtrag des Koordinators:** [A7S05](../review-dispositions.md#a7s05); ursprüngliche Abgabe unverändert.

**Architekturfolge für diesen Stressor:** Signaturherkunft von Verhaltensintegrität trennen. Historie Beobachter und Eingriffsautorität nur bei wirklich getrennten Vertrauensgrenzen als Gegenbezug werten und einzigartige gelöschte Wahrheit nicht erfinden.
