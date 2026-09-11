<!-- Generated exclusively by docs/residuality/a4/build.py. -->
# 50 Suchstrategien

Drei kuratierte Gegenproben pro Strategie. Keine Zufallsstichprobe oder Normzertifizierung.

| ID | Qualitätsziel | Suchbewegung | Leitfrage | Stressoren | Quellen |
|---|---|---|---|---|---|
| Q01 | Verfügbarkeit | Den ganzen Rechner verschwinden lassen | Wer bemerkt den Ausfall wenn Factory selbst schweigt | S201 S202 S203 | B02 B06 Nutzerfrage |
| Q02 | Unabhängigkeit | Gemeinsame Ausfallquellen suchen | Teilen Rechner Beobachter und Alarmweg doch Strom Netz oder Account | S204 S205 S206 | B02 B06 |
| Q03 | Beobachtbarkeit | Den Beobachter ausschalten | Wer entdeckt eine kaputte Alarmkette statt nur kaputte Worker | S207 S208 S209 | B06 |
| Q04 | Diagnostizierbarkeit | Grünmeldung gegen echten Fortschritt stellen | Welche gesund aussehenden Komponenten leisten keine nützliche Arbeit | S210 S211 S212 | B02 |
| Q05 | Aktualität | Beobachtungen zeitversetzen und wiederholen | Kann ein altes Lebenszeichen einen heutigen Ausfall verdecken | S213 S214 S215 | B02 A3 |
| Q06 | Messökonomie | Den Messapparat selbst überlasten | Wann verursacht Monitoring die Störung die es beobachten soll | S216 S217 S218 | B02 B03 |
| Q07 | Zeitverhalten | Das Latenzbudget durch alle Schichten verfolgen | Kann jede Teilfrist passen während die Gesamtfrist längst abgelaufen ist | S219 S220 S221 | B03 B04 |
| Q08 | Messgültigkeit | Die unsichtbaren langsamen Anfragen zählen | Welche Wartenden Abbrüche und nie gestarteten Aufgaben fehlen im Histogramm | S222 S223 S224 | B02 eigene Gegenprobe |
| Q09 | Skalierung | Den langsamsten Ast eines Fan-outs vergrößern | Was geschieht wenn ein Auftrag tausende heterogene Antworten braucht | S225 S226 S227 | B04 |
| Q10 | Tail-Latenz | Eine winzige dauerhaft hängende Teilpopulation einführen | Kann fast alles schnell sein und trotzdem alle Plätze belegt bleiben | S228 S229 S230 | B03 B04 |
| Q11 | Wirkungssicherheit | Antworten nach dem Vergessen eintreffen lassen | Was passiert wenn Antwort oder Wirkung später kommt als die lokale Historie reicht | S231 S232 S233 | A3 B03 |
| Q12 | Abbrechbarkeit | Abbruch und Außenwirkung gegeneinander laufen lassen | Was bedeutet Cancel wenn der andere Teilnehmer schon angenommen hat | S234 S235 S236 | B03 A3 |
| Q13 | Stabilität | Retries auf allen Ebenen multiplizieren | Wie wird eine kleine Störung durch Wiederholungen zur Lastlawine | S237 S238 S239 | B03 |
| Q14 | Anlaufverhalten | Alle warmen Zustände gleichzeitig entfernen | Ist ein kalter Start noch tragfähig oder nur der eingespielte Betrieb | S240 S241 S242 | B03 |
| Q15 | Fairness | Eine unendlich lange Aufgabe vor kurze Aufgaben setzen | Wer verhungert durch Reihenfolge Priorität oder blockierte Reservierungen | S243 S244 S245 | B03 eigene Gegenprobe |
| Q16 | Kapazität | Jede Mengendimension einzeln um Größenordnungen erhöhen | Welches Limit bricht zuerst bei IDs Bytes Prozessen oder Abonnenten | S246 S247 S248 | B01 B03 |
| Q17 | Langzeitbetrieb | Jahre an Geschichte in ein kleines Recoveryfenster pressen | Ist ein replaybares System noch rechtzeitig und mit realem Speicher aufbaubar | S249 S250 S251 | A3 B01 |
| Q18 | Elastizität | Lange Stille in einen synchronen Nachfragepuls verwandeln | Was geschieht wenn alle verpassten Termine und Nutzer gleichzeitig zurückkommen | S252 S253 S254 | B03 |
| Q19 | Energieeffizienz | Thermik und Strom als geteilte Ressource behandeln | Welche Annahmen zerbrechen bei Drosselung Batterieende oder Lastabwurf | S255 S256 S257 | B01 eigene Gegenprobe |
| Q20 | Steuerbarkeit | Im Notfall den Kontrollpfad benötigen | Bleibt Stop und Diagnose gerade unter maximalem Druck verfügbar | S258 S259 S260 | B02 A3 |
| Q21 | Security und Autorisierung | Einen berechtigten Vermittler für fremde Ziele missbrauchen | Kann ein gültiger Actor den falschen Scope oder Empfänger handeln lassen | S261 S262 S263 | B05 |
| Q22 | Instruktionsintegrität | Daten als scheinbare Befehle verkleiden | Werden Inhalte Logs oder Toolantworten ungewollt zu Autorität | S264 S265 S266 | B05 eigene Agent-Gegenprobe |
| Q23 | Lieferkettensicherheit | Korrekte Signatur mit bösartigem Inhalt kombinieren | Welche Sicherheit bleibt wenn Herkunft stimmt aber der Lieferant kompromittiert ist | S267 S268 S269 | B05 A3 |
| Q24 | Schlüsselkontinuität | Rotation Wiederherstellung und Kryptoperioden kollidieren lassen | Was geschieht wenn alte Belege und heutige Schlüssel nicht mehr zusammenpassen | S270 S271 S272 | B05 A3 |
| Q25 | Isolation | Kooperative Regeln durch feindliche Gleichzeitigkeit ersetzen | Was kann ein Mitnutzer über OS Speicher Zeit oder Zugangsdaten umgehen | S273 S274 S275 | B05 A3 |
| Q26 | Datenschutz | Aus harmlosen Messwerten sensible Zusammenhänge rekonstruieren | Welche Information verraten Labels Heartbeats und Exporte zusammengenommen | S276 S277 S278 | B01 B05 |
| Q27 | Datenintegrität | Gültig aussehende gemeinsame Beschädigung erzeugen | Wie wird ein Fehler bemerkt den Prüfsumme Kopie und Prüfer gemeinsam teilen | S279 S280 S281 | A3 eigene Gegenprobe |
| Q28 | Wiederherstellbarkeit | Eine alte Welt in die heutige Außenwelt zurücksetzen | Welche längst erledigten oder verbotenen Aktionen erscheinen wieder offen | S282 S283 S284 | A3 |
| Q29 | Kompatibilität | Alle Versionskombinationen statt nur latest gegen latest betrachten | Wo zerbricht ein formal gültiger Vertrag semantisch | S285 S286 S287 | B01 A3 |
| Q30 | Änderbarkeit | Rollback nach irreversibler Vorwärtswirkung verlangen | Welche Änderung kann ein altes Binary nicht rückgängig machen | S288 S289 S290 | B01 A3 |
| Q31 | Wartbarkeit | Den Wissenden und seine Werkzeuge entfernen | Kann jemand ohne Maintainer Cloudkonto und alte Toolchain sicher reparieren | S291 S292 S293 | B01 A3 |
| Q32 | Modularität | Eine kleine Änderung durch den Abhängigkeitsgraphen schicken | Wie groß ist der tatsächliche statt behauptete Ausfallradius | S294 S295 S296 | B01 |
| Q33 | Testbarkeit | Den Testapparat zum Mitverursacher machen | Welche Fehler verschwinden nur weil der Test Scheduler Uhr und Reihenfolge kontrolliert | S297 S298 S299 | B01 eigene Gegenprobe |
| Q34 | Funktionale Richtigkeit | Transporterfolg mit falscher Bedeutung kombinieren | Was passiert bei falscher Einheit Zielperson Rundung oder Teilantwort | S300 S301 S302 | B01 A3 |
| Q35 | Bedienbarkeit und Zugänglichkeit | Unter Stress ohne übliche Anzeige handeln | Sind Zuständigkeit Unsicherheit und Folgen ohne Farbe Maus oder Muttersprache erkennbar | S303 S304 S305 | B01 B02 |
| Q36 | Betriebsfähigkeit | Den Menschen in den langen Latenzschwanz setzen | Was bleibt wenn jede technische Prüfung schnell ist aber niemand entscheidet | S306 S307 S308 | B02 A3 |
| Q37 | Verantwortbarkeit | Mehrere legitime Nachfolger mit Konflikt erzeugen | Wer darf während Nachfolge Befangenheit oder Vertretung tatsächlich handeln | S309 S310 S311 | A3 eigene Gegenprobe |
| Q38 | Austauschbarkeit | Unabhängige Anbieter auf ihren gemeinsamen Eigentümer zurückführen | Wie viele Ausfalldomänen bleiben hinter Marken Accounts und Zahlungssystemen | S312 S313 S314 | A3 eigene Gegenprobe |
| Q39 | Kostenkontrolle | Kosten später als die Budgetentscheidung sichtbar machen | Kann ein scheinbar billiger Auftrag bereits ein großes offenes Kostenrisiko besitzen | S315 S316 S317 | B01 A3 |
| Q40 | Rechtliche Steuerbarkeit | Widersprechende Mandate durch große Verzögerungen schicken | Welche Aktion ist bei Annahme anders erlaubt als bei Auftrag oder Urteil | S318 S319 S320 | A3 eigene Gegenprobe |
| Q41 | Safety | Eine Softwareaktion an irreversible physische Folgen koppeln | Wo ist später stoppen oder kompensieren fachlich unzureichend | S321 S322 S323 | B01 A3 |
| Q42 | Netztoleranz | Asymmetrische und tagelange Partitionen statt Totalausfall erzeugen | Was sieht jeder Teilnehmer wenn nur eine Richtung und nur manchmal funktioniert | S324 S325 S326 | B03 eigene Gegenprobe |
| Q43 | Zeitrobustheit | Die Uhrgeometrie verändern | Welche Frist überlebt Suspend Uhrsprung Datumsgrenze oder lange Isolation | S327 S328 S329 | A3 |
| Q44 | Erkenntnisqualität | Widersprüchliche Zeugen ohne gemeinsamen Richter zulassen | Welche Behauptung kann Factory wirklich belegen statt nur auswählen | S330 S331 S332 | A3 eigene Gegenprobe |
| Q45 | Anreizfestigkeit | Die Messzahl zum Optimierungsziel machen | Kann ein Agent schneller grüner oder billiger erscheinen indem er Nutzen versteckt | S333 S334 S335 | B02 eigene Gegenprobe |
| Q46 | Nachhaltigkeit | Sparziele gegen Verfügbarkeit und Fristen stellen | Welche legitimen Qualitätsziele lassen sich nicht gleichzeitig maximieren | S336 S337 S338 | B01 eigene Gegenprobe |
| Q47 | Portabilität | Auf fremder Hardware und nach Jahrzehnten neu anfangen | Was bleibt ohne identische Architektur Pfadsemantik und Laufzeit | S339 S340 S341 | B01 A3 |
| Q48 | Beendbarkeit | Das System sauber verlassen wollen | Was muss beim Ende noch nachweisbar sein obwohl niemand weiterbetreiben will | S342 S343 S344 | A3 |
| Q49 | Grenzprüfung | Alle unabhängigen Anker zugleich entziehen | Welche Versprechen werden logisch unmöglich statt nur teuer | S345 S346 S347 | eigene bewusste Extremprobe |
| Q50 | Sequenzangriff | Dem Gegner die Reihenfolge der Störungen überlassen | Welche sichere Einzelmaßnahme wird durch eine zweite und dritte Störung gefährlich | S348 S349 S350 | B03 eigene Gegenprobe |
