# Warum Factory so gebaut werden soll

**Aktuelle vollständige Herleitung:** [A7 behandelt alle 350 Stressoren](a7/README.md) mit konkreten bedingten Zuständen, Residue-Dispositionen und einer [überarbeiteten modularen Zielarchitektur](a7/zielarchitektur.md). Dieses ältere Dokument bleibt eine Erklärung der damaligen A3/A4-Kategorien, nicht die aktuelle Gesamtzuordnung.

**Einordnung nach dem unabhängigen Review A6:** Dieses Dokument erklärt die bisherigen A3/A4-Entwurfskategorien. Die Zahl 17 ist **keine nachgewiesene vollständige Menge von Residues**. Die [Zustandsanalyse A5](a5/README.md) und der neuere [Review durch fünf Subagenten](a6/README.md) untersuchen zuerst Verläufe, Rückkopplungen, dauerhaft verbleibende Zustände und Gegenbeispiele. A6 korrigiert mehrere frühere Modellinterpretationen. Auch die unabhängigen Analysen sind noch kein empirischer Nachweis tatsächlicher Factory-Attraktoren oder überlebender Strukturen.

## Die Herleitung in einfachen Worten

Wir haben inzwischen **350 mögliche Situationen** betrachtet: die ersten 200 plus 150 weitere aus **50 unterschiedlichen Suchstrategien**, etwa Security, Skalierung, Beobachtbarkeit und Wartbarkeit. Die wichtige Frage war nicht nur: **„Was kann kaputtgehen?“**, sondern: **„Was muss dann noch funktionieren?“**

Eine solche verbleibende Fähigkeit nennen wir **Residue**. Beispiel: Der Agent ist nicht erreichbar, aber sein Arbeitsverzeichnis bleibt vor einem zweiten Agenten geschützt.

Die Kette lautet immer:

> **Das könnte passieren → deshalb muss diese Fähigkeit erhalten bleiben → deshalb bauen wir Factory nach dieser konkreten Regel.**

Unten stehen alle **17 Residues** mit ausgewählten, konkret benannten Stressoren. Mehrere Stressoren können zum selben Residue führen. Umgekehrt kann ein Stressor mehrere Residues brauchen. Wir bauen deshalb **nicht 200 Fehlerbehandlungen und auch nicht 16 neue Dienste**, sondern verstärken einige gemeinsame Teile von Factory.

**Wichtig:** Das erklärt A3 samt der Erweiterung A4. Neu ist insbesondere die unabhängige Ausfallmeldung in Abschnitt 17. „Entscheidung“ bedeutet hier: *So soll der Entwurf aussehen.* Nicht: *Das ist bereits vollständig umgesetzt oder getestet.* Die Nummern E01–E17 sind nur Lesehilfen, keine neuen Architekturentscheidungsdokumente.

---

## 1. Nicht erreichbar heißt nicht beendet

**Stressoren:** Herdr antwortet nicht, während der Agent weiter Dateien schreibt (S031). Nach einem Neustart gehört dieselbe Terminal-ID plötzlich zu einem anderen Agenten (S032).

**Warum führt das zum Residue?** Aus fehlender oder falscher Beobachtung darf kein zweiter Agent Schreibzugriff bekommen. Erhalten bleiben muss also die **geschützte Belegung des Arbeitsverzeichnisses — R01**.

**Entscheidung E01:** Factory behält die Reservierung, solange das Ende des alten Agenten unklar ist. Vor einer Wiederaufnahme muss es prüfen: Ist es wirklich derselbe Agent? Lebt er? Ist die Beobachtung zuverlässig und aktuell? Eine fehlende ID zählt nicht als Übereinstimmung.

**Konkrete Folge:** Der Recovery-Code darf die Reservierung nicht allein wegen eines Timeouts freigeben. Ein Mensch kann den Fall ausdrücklich auflösen; seine Entscheidung ist aber nicht automatisch ein Beweis, dass der alte Prozess beendet ist.

## 2. Keine Antwort heißt nicht: Die Aktion ist nicht passiert

**Stressoren:** Strom fällt direkt nach einer Übergabe aus (S001). Ein externer Dienst nimmt eine Aktion an, aber seine Antwort geht verloren (S051).

**Warum führt das zum Residue?** Nach dem Neustart muss noch erkennbar sein, was möglicherweise schon ausgelöst wurde. Erhalten bleiben muss ein **dauerhaftes Gedächtnis der beabsichtigten und versuchten Aktion — R02**.

**Entscheidung E02:** Factory speichert die Aktion mit eindeutiger Kennung **vor** ihrer Ausführung. Danach speichert es das Ergebnis. Fehlt die Bestätigung, lautet der Zustand „Ausgang unbekannt“, nicht „sicher fehlgeschlagen“.

**Konkrete Folge:** Erst beim Dienst nachprüfen, dann über eine Wiederholung entscheiden. Kann der Dienst keine Auskunft geben, bleibt menschliche Klärung nötig. Eine Email darf nicht allein wegen einer verlorenen Antwort nochmals gesendet werden.

## 3. Es darf nicht zwei gleichzeitig zuständige Zentralen geben

**Stressoren:** Zwei Kopien derselben Factory laufen auf verschiedenen Macs (S065). Alter Python-Prototyp und neue Rust-Version verändern gleichzeitig dieselben Geschäftsdaten (S153).

**Warum führt das zum Residue?** Beide können für sich korrekt arbeiten und trotzdem widersprüchliche Entscheidungen treffen. Erhalten bleiben muss eine **eindeutige Zuständigkeit für Änderungen — R03**.

**Entscheidung E03:** Pro Installation verändert nur ein zentraler Factory-Prozess die Kerndaten. Beim Wechsel vom Prototyp zur neuen Version wird die Zuständigkeit ausdrücklich übergeben, nicht parallel ausgeübt.

**Konkrete Folge:** Eine lokale Sperre verhindert einen zweiten lokalen Prozess. Zwei Macs brauchen zusätzlich einen wirklichen Ausschluss alter Zugriffe beim externen Dienst — oder einen bewusst manuell gesicherten Betrieb mit nur einer aktiven Kopie. Eine neue ID allein genügt nicht.

## 4. Die Geschichte darf nicht von der heutigen Anzeige abhängen

**Stressoren:** Beim Wiederaufbau werden versehentlich heutige Einstellungen für alte Vorgänge verwendet (S018). Ein benötigtes Ergebnisdokument wurde gelöscht (S016).

**Warum führt das zum Residue?** Eine aktuelle Statuszeile erklärt nicht, wie der Zustand entstanden ist. Erhalten bleiben muss die **nachvollziehbare Geschichte samt benötigten Inhalten — R04**.

**Entscheidung E04:** Fachliche Veränderungen werden als unveränderliche Ereignisse gespeichert. Statusansichten lassen sich daraus wieder aufbauen. Dabei werden gespeicherte damalige Werte verwendet, nicht heutige Dateien abgefragt.

**Konkrete Folge:** Dieser Wiederaufbau startet keine Agenten und wiederholt keine externen Aktionen. Fehlt ein notwendiger Inhalt, stoppt er mit einer klaren Fehlermeldung. Ein gespeicherter Datei-Hash ersetzt die verschwundene Datei nicht.

## 5. Ein Auftrag muss ohne das alte Terminal verständlich sein

**Stressoren:** Der Zielagent wird umbenannt (S061). Der Modellanbieter verschwindet (S101). Die Arbeitsanweisung ändert sich zwischen Auftragserteilung und Start (S111).

**Warum führt das zum Residue?** Der Auftrag darf nicht nur im Gesprächsgedächtnis eines bestimmten Agenten existieren. Erhalten bleiben muss ein **übergebbares Arbeitspaket — R05**.

**Entscheidung E05:** Factory speichert beim Lauf den vorgesehenen Empfänger, den Auftrag, die verwendeten Versionen der Eingaben und Anweisungen sowie die Kriterien für ein brauchbares Ergebnis. Sensible Inhalte werden nur über dafür geeignete geschützte Verweise eingebunden.

**Konkrete Folge:** Ein anderer berechtigter Agent kann verstehen, was zu tun war. Ein Wechsel wird ausdrücklich dokumentiert. Alte Anweisungen setzen dabei keine heute geltenden Verbote außer Kraft. Übergebbarkeit garantiert noch keine gleiche Ergebnisqualität.

## 6. Eine defekte Erweiterung soll nicht alles mitreißen

**Stressoren:** Ein Plugin schreibt Debugtext in den Datenkanal (S041). Es beantwortet Lebenszeichen, hängt aber bei jeder eigentlichen Aufgabe (S043).

**Warum führt das zum Residue?** Der Rest von Factory soll nicht auf dieselbe Erweiterung warten müssen. Erhalten bleiben muss ein **Betrieb ohne die defekte Integration — R06**.

**Entscheidung E06:** Plugins laufen als getrennte, überwachte Prozesse. Sie erhalten Grenzen für Laufzeit, Datenmenge und Neustartversuche. Nach wiederholten Fehlern wird die betroffene Erweiterung gesperrt, nicht endlos neu gestartet.

**Konkrete Folge:** Ein defektes Versandplugin soll andere Projekte nicht lahmlegen. Seine möglicherweise bereits ausgelösten Aktionen bleiben trotzdem nachverfolgbar. Ein Prozess ist eine Fehlergrenze, aber noch keine vollständige Sicherheitsisolierung.

## 7. Factory muss rechtzeitig Nein sagen können

**Stressoren:** Die SSD wird voll (S011). Ein lokales Modell belegt den ganzen Speicher (S106). Ein Projekt beansprucht alle freien Plätze (S143).

**Warum führt das zum Residue?** Unbegrenzte Aufnahme zerstört irgendwann auch die Fähigkeit, laufende Arbeit sicher zu verwalten. Erhalten bleiben muss **ein begrenzter, noch steuerbarer Teil des Betriebs — R07**.

**Entscheidung E07:** Neben Grenzen pro Agent gibt es gemeinsame Grenzen für Aufgaben, Prozesse, Speicher, Datenmengen und Kosten. Ein Teil der Kapazität bleibt für Kontrolle reserviert. Auch Wiederholungen über mehrere Schichten und gleichzeitig gestartete Unteraufgaben teilen ein Budget. Sonst kann ein kleiner Fehler eine Lastlawine auslösen (S237).

**Konkrete Folge:** Neue Arbeit wird zurückgestellt oder abgelehnt, bevor alles erschöpft ist. Projekte teilen die Kapazität nach einer festgelegten Regel. Factory löscht nicht still seine wichtige Geschichte, nur um weiterarbeiten zu können. Die konkreten Grenzwerte müssen wir noch vereinbaren.

## 8. Auch ein kaputtes Factory muss erklärbar bleiben

**Stressoren:** Eine optionale Plugin-Konfiguration ist beschädigt (S045). Der HTTP-Zugang fällt aus, der Rechner funktioniert aber noch (S055).

**Warum führt das zum Residue?** Zur Diagnose darf nicht ausgerechnet die kaputte Erweiterung benötigt werden. Erhalten bleiben muss **ein kleiner lokaler Kontrollzugang — R08**.

**Entscheidung E08:** Factory bekommt einen Minimalbetrieb ohne optionale Plugins. Wenn der zentrale Prozess nicht läuft, darf ein Diagnosewerkzeug die Daten ausdrücklich nur lesen.

**Konkrete Folge:** Ein Mensch kann Fehler und offene Aufgaben untersuchen, ohne zuerst alle Integrationen zu reparieren. Das Diagnosewerkzeug wird nicht zu einer zweiten schreibenden Zentrale. Bei defektem Rechner hilft allerdings auch dieser Zugang nicht mehr: Die **Ausfallmeldung braucht R17**, der spätere **Wiederaufbau R11**. Außerdem gilt: Ein antwortender Host ist nicht automatisch ein arbeitender Factory-Kernel (S202/S210).

## 9. Eine Freigabe gilt für eine konkrete Aktion

**Stressoren:** Ein Mensch genehmigt Rechnung A, danach wird der Auftrag auf Rechnung B geändert (S083). Eine Freigabe wird widerrufen, nachdem der Dienst die Aktion schon angenommen hat (S084).

**Warum führt das zum Residue?** „Dieser Task wurde irgendwann genehmigt“ ist zu ungenau. Erhalten bleiben muss eine **klare Grenze dessen, was jetzt ausgeführt werden darf — R09**.

**Entscheidung E09:** Die Freigabe nennt die Person, den genauen Inhalt, den Empfänger, die Version und ihre Gültigkeit. Unmittelbar vor der Aktion wird sie erneut geprüft.

**Konkrete Folge:** Veränderte Rechnung, veränderter Empfänger oder abgelaufene Freigabe: kein neuer Versand ohne passende Erlaubnis. Die Erlaubnis, einen Prompt erneut zuzustellen, ist keine pauschale Erlaubnis, dessen Geschäftsaktionen zu wiederholen. Ein Widerruf kann eine bereits gesendete Email nicht zurückholen.

## 10. Der Mensch ist keine unbegrenzt verfügbare Infrastruktur

**Stressoren:** Der einzige Operator ist drei Tage nicht erreichbar (S091). Er bestätigt im Alarmsturm alles reflexhaft (S092). Er fällt dauerhaft aus (S161).

**Warum führt das zum Residue?** „Im Zweifel fragt das System einen Menschen“ funktioniert nur, wenn jemand zuständig und tatsächlich entscheidungsfähig ist. Erhalten bleiben muss **eine überschaubare, legitim übergebbare Entscheidungsliste — R10**.

**Entscheidung E10:** Offene Entscheidungen werden mit Grund, betroffener Aktion, Optionen und Folgen dargestellt und priorisiert. Vertretung und Nachfolge werden ausdrücklich festgelegt.

**Konkrete Folge:** Ein Vertreter sieht, was offen ist und was er entscheiden darf. Gibt es niemanden mit dieser Befugnis, bleibt die Aktion stehen. Schweigen wird nicht nach einiger Zeit als Zustimmung gewertet.

## 11. Ein Backup muss auch ohne den alten Rechner helfen

**Stressoren:** Ein Brand zerstört Mac und danebenliegende Backupplatte (S006). Die Backups sind vorhanden, aber der Schlüssel fehlt (S017). Sämtliche Kopien und Schlüssel gehen verloren (S190).

**Warum führt das zum Residue?** Eine Kopie auf demselben Gerät schützt nicht vor Geräteverlust. Erhalten bleiben muss **eine unabhängig wiederherstellbare Installation — R11**.

**Entscheidung E11:** Der Wiederaufbau braucht eine genehmigte Kopie außerhalb derselben Ausfallquelle: Daten, benötigte Inhalte, Versionsinformationen und Anleitung. Der Zugang zu Schlüsseln wird separat geregelt. Das Verfahren wird auf einer anderen Umgebung geübt.

**Konkrete Folge:** Nicht nur „Backup erfolgreich“ melden, sondern beweisen, dass ein Wiederaufbau möglich ist. Danach werden alte Aufträge nicht blind gestartet: Seit dem Backup könnte bereits etwas passiert sein. Sind alle Informationen verloren, gibt es keine technische Wiederherstellung.

## 12. Die Geschichte darf nicht zum dauerhaften Geheimnisleck werden

**Stressoren:** Ein Secret landet im Prompt (S113). Eine Person verlangt Löschung ihrer Daten (S131). Ein für den Wiederaufbau benötigter Inhalt muss rechtlich gelöscht werden (S194).

**Warum führt das zum Residue?** Alles für immer zu speichern schafft selbst ein Risiko. Erhalten bleiben muss **eine nachvollziehbare Geschichte mit begrenzter Datenhaftung — R12**.

**Entscheidung E12:** Das dauerhafte Ereignisprotokoll enthält möglichst wenige, nicht geheime Angaben. Sensible Inhalte liegen getrennt unter geschützten Verweisen. Secrets gehören in den vorgesehenen sicheren Speicher, nicht in Prompts und Logs.

**Konkrete Folge:** Für Inhalte gibt es bewusste Aufbewahrungs- und Löschregeln. Wenn zulässige Löschung eine vollständige Rekonstruktion verhindert, wird diese Grenze offengelegt. Factory entscheidet rechtliche Widersprüche nicht selbst. Bereits abgeflossene Daten werden durch einen Korrektureintrag nicht zurückgeholt.

## 13. Ein alter Termin ist nicht automatisch ein heute sinnvoller Auftrag

**Stressoren:** Die Zeitumstellung erzeugt dieselbe lokale Uhrzeit zweimal (S021). Nach sechs Wochen Pause sind tausende Termine offen (S024). Ein im Auftrag genannter Preis ist inzwischen abgelaufen (S028).

**Warum führt das zum Residue?** „Die Uhr hat diesen Wert“ sagt noch nicht, welche Arbeit heute fällig und gültig ist. Erhalten bleiben muss **eine eindeutige Bedeutung von Termin und Gültigkeit — R13**.

**Entscheidung E13:** Jeder geplante Termin bekommt eine eindeutige Kennung. Der Zeitplan legt ausdrücklich fest, was bei Zeitumstellung, Ausfall und Nachholen geschieht. Die Gültigkeit von Eingaben und Freigaben wird zusätzlich geprüft. Zur Gesamtfrist zählt auch die Queuezeit: Acht Teilaufrufe dürfen nicht jeweils eine neue volle Frist bekommen (S219/S220).

**Konkrete Folge:** Factory holt nicht ungefragt sechs Wochen externer Aktionen nach. Es kann veraltete Arbeit sichtbar zurückhalten oder nach der vereinbarten Regel verfallen lassen. Technische Wartefristen werden nicht einfach von einer verstellbaren Kalenderuhr abhängig gemacht. Kommt eine Antwort erst nach Monaten, gehört sie weiterhin zum alten Vorgang — sie erlaubt keine neue Aktion (S231). Und eine gute Latenzanzeige muss auch die noch Wartenden zeigen, nicht nur schnelle fertige Aufgaben (S222).

## 14. „Fertig“ ist nicht dasselbe wie „richtig“

**Stressoren:** Der Harness meldet nach einem Zwischenschritt `done` (S035). Zwei Prüfer mit demselben Modell bestätigen denselben Fehler (S104). Im Extremfall teilen alle Prüfer denselben Irrtum (S184).

**Warum führt das zum Residue?** Eine Abschlussmeldung oder Mehrheit ist kein zuverlässiger Qualitätsbeweis. Erhalten bleiben muss **ein prüfbares Ergebnis mit erkennbarem Maßstab — R14**.

**Entscheidung E14:** Factory trennt drei Aussagen: Der Worker hat abgegeben. Das Ergebnis wurde geprüft. Die externe Verwendung ist genehmigt. Die Prüfung nennt ihre Kriterien und Herkunft.

**Konkrete Folge:** Ein erfolgreich beendeter Prozess versendet noch keinen fachlich ungeprüften Bericht. Wo möglich prüfen ausführbare Tests das Ergebnis; sonst braucht es einen geeigneten menschlichen Maßstab. Mehr identische Prüfer sind nicht automatisch unabhängig. Allgemeine Fehlerfreiheit wird nicht versprochen.

## 15. Nicht jede neue Nutzung passt in dieselbe Sicherheitsarchitektur

**Stressoren:** Ein Plugin liest als derselbe Betriebssystemnutzer Daten außerhalb seiner vorgesehenen Rechte (S121). Zwei Kunden verlangen gegenseitige Geheimhaltung auf demselben Nutzerkonto (S172).

**Warum führt das zum Residue?** Projektregeln und getrennte Prozesse sind keine harte Mandantentrennung. Erhalten bleiben muss **ein ehrliches, begrenztes Einsatzversprechen — R15**.

**Entscheidung E15:** Version 1 bleibt ein System für einen vertrauenswürdigen Betreiber mit vertrauenswürdigen Workloads. Feindliche Plugins oder voneinander zu isolierende Kunden werden nicht als bereits sicher unterstützte Nutzung behandelt.

**Konkrete Folge:** Solche Anwendungen brauchen zuerst ein eigenes Konzept mit wirksamen Betriebssystem-, VM- und Zugangsdaten-Grenzen. Zusätzliche Promptregeln reichen dafür nicht. Factory wird lieber für einen Zweck nicht eingesetzt, als dort falsche Sicherheit zu versprechen.

## 16. Factory muss auch wechselbar und abschaltbar bleiben

**Stressoren:** Prototyp und Nachfolger verwenden inkompatible Datenmodelle (S153). Eine benötigte Softwareversion ist nicht mehr erhältlich (S155). Ein Kunde will seine Daten exportieren und Factory abschalten (S179).

**Warum führt das zum Residue?** Die Arbeit darf nicht untrennbar an die aktuelle Software oder ihren Maintainer gebunden sein. Erhalten bleiben muss **eine wartbare und verlassbare Installation — R16**.

**Entscheidung E16:** Datenformate und Softwarestände sind eindeutig versioniert. Umstellungen haben einen dokumentierten Übergabe- und Rückkehrplan. Ein lesbarer Export und ein geordneter Ausstieg gehören zum Entwurf.

**Konkrete Folge:** Eine ältere Softwareversion darf ein unbekanntes neueres Datenformat nicht still verändern. Beim Wechsel wird keine fehlende Vergangenheit erfunden. Beim Abschalten bleiben noch mögliche externe Wirkungen und Aufbewahrungspflichten sichtbar.

---

## 17. Der ausgefallene Rechner kann seinen Ausfall nicht selbst melden

**Stressoren:** Der ganze Mac verliert Strom (S201). Das Monitoring hängt an derselben Steckdose und fällt gleich mit aus (S204). Alte gepufferte Lebenszeichen lassen einen toten Rechner scheinbar gesund aussehen (S213). Oder der Alarm wird erzeugt, erreicht aber niemanden (S208/S306).

**Warum führt das zum Residue?** Lokale Diagnose und Backup helfen nicht dabei, den Ausfall überhaupt zu bemerken. Erhalten bleiben muss eine **vom Factory-Rechner unabhängige Ausfallmeldung — R17**.

**Entscheidung E17:** Ein externer Beobachter erwartet regelmäßig ein aktuelles Lebenszeichen. Bleibt es aus, meldet **dieser Beobachter** den Ausfall über einen ebenfalls unabhängigen, vorher genehmigten Weg an Betreiber oder Vertretung. Alte Meldungen dürfen keine aktuelle Gesundheit vortäuschen.

**Konkrete Folge:** Der Alarm funktioniert auch bei ausgeschaltetem Mac. Wir testen nicht nur den Monitor, sondern die gesamte Kette bis zum Empfänger — einschließlich des Ausfalls des Monitors selbst. Wartungsstummschaltungen müssen enden. Gemeldet wird „kein aktuelles Lebenszeichen“, nicht vorschnell „Rechner kaputt“. Der Monitor darf deshalb weder einen Ersatzagenten starten noch eine Aktion wiederholen.

**Grenze:** Fallen alle Beobachter, Kommunikationswege und erreichbaren Menschen gemeinsam aus, kommt kein Alarm (S345). Ein externer Dienst ist noch nicht ausgewählt oder eingerichtet; Anbieter, Daten, Empfänger, Kosten und Zeitgrenzen brauchen vorher eine Freigabe.

---

## Welche Architektur ergibt sich daraus insgesamt?

```text
Mensch oder Agent gibt einen Auftrag
                 ↓
Eine zentrale Factory prüft:
Wer darf das? Ist es noch gültig? Ist genug Kapazität da?
                 ↓
Sie speichert Auftrag, Entscheidung und geplante Aktion dauerhaft.
                 ↓
Eine begrenzte, überwachte Erweiterung führt die Aktion aus.
                 ↓
Factory erfasst, was tatsächlich bekannt ist.
Unklar bleibt unklar. Fertig ist noch nicht geprüft.
```

Dazu kommen vier Sicherheitsnetze:

1. **Geschützte Arbeitsverzeichnisse:** Kein zweiter Agent, nur weil der erste gerade unsichtbar ist.
2. **Kleiner lokaler Kontrollzugang:** Fehler verstehen und Arbeit begrenzen, auch wenn Erweiterungen ausfallen.
3. **Unabhängige Ausfallmeldung:** Ein anderer Dienst bemerkt das Schweigen von Factory und erreicht den Betreiber ohne den ausgefallenen Rechner.
4. **Unabhängiger Wiederaufbau und Übergabe:** Arbeit darf weder am alten Rechner noch am alten Modell oder Maintainer hängen.

Die Analyse bestätigt damit einige bestehende Entscheidungen — etwa eine zentrale schreibende Instanz und getrennte Plugins. Sie verschärft andere — insbesondere Recovery, konkrete Freigaben und gemeinsame Ressourcenlimits. Sie verlangt **keinen neuen Dienst für jedes Residue**.

## Was davon steht schon, was noch nicht?

Im untersuchten Arbeitsstand gibt es unter anderem Datenbanktransaktionen, Workspace-Reservierungen und das Speichern eines Zustellversuchs vor der Übergabe. Die vollständige Ereignishistorie, der vollständige Pluginbetrieb und die hier beschriebenen Ende-zu-Ende-Garantien sind damit noch nicht geliefert. Gerade die Behandlung fehlender Runtime-Evidenz muss gezielt überprüft werden.

Dieses Dokument erklärt also **das Warum und das konkrete Soll**, nicht einen abgeschlossenen Umbau. Budgetwerte, Vertretungsrechte, akzeptabler Datenverlust und fachliche Prüfkriterien müssen noch ausdrücklich festgelegt werden. Ob die Regeln praktisch tragen, müssen anschließend Tests zeigen.

## Falls du eine einzelne Herleitung nachprüfen möchtest

Die S- und R-Nummern stammen unverändert aus der großen Analyse. Die Beispiele oben sind eine Auswahl, keine vollständige Aufzählung aller Verbindungen.

- [Die ersten 200 Stressoren mit Beschreibung und Residue-Zuordnung](generated/stressors.md)
- [Die 150 neuen Stressoren aus 50 Strategien](a4/generated/stressors.md)
- [Alle Einzelverbindungen der 350 Karten: Stressor → Residue → konkrete Regel](a4/generated/traceability.csv)
- [Technische Basis A3](architecture-A3.md) und [Erweiterung A4 mit externer Überwachung und Qualitätsregeln](a4/architecture-A4.md)

**Kurz gesagt: Die Stressoren zeigen, welche Annahmen brechen. Die Residues sagen, was dann übrig bleiben muss. Die Architekturentscheidungen legen fest, wie Factory genau diese Fähigkeiten erhält — und wo es ehrlich stoppen muss.**
