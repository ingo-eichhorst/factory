# A7 — Vorgehen und Grenzen

## Auftrag

Alle 350 Stressoren erhalten explizite bedingte Zustands- und Residue-Zuordnungen. Daraus entsteht eine modulare Zielarchitektur. Es gibt keine vorgegebene Zahl von Residues und keine Pflicht, jede Störung zu einem Attraktor umzudeuten.

## Reihenfolge

1. Vier neue Task-Sessions in den vorhandenen getrennten Reviewscopes analysieren S001–S100, S101–S200, S201–S275 und S276–S350. Die früheren Originalberichte werden nicht verändert.
2. Eingaben sind die neutralen Szenariotexte, die eigenen A6-Trajektorien und die abgeschlossenen skeptischen Korrekturen. Die Analysten lesen keine neuen A7-Peer-Ergebnisse vor Abgabe. Diese Runde ist nicht erneut blind gegenüber A6, sondern eine ausdrücklich darauf aufbauende Herleitung.
3. Jeder Fall erhält einen oder mehrere begründete Zweige. Jeder Zweig benennt Zustandsart, Voraussetzungen, Fortbestand/Ausstieg, konkrete nutzbare Reste und Gegenprüfung. Keine gewünschte Schutzmaßnahme wird als bereits vorhandenes Produktfeature ausgegeben.
4. Konkrete Residue-Kandidaten werden pro Analyst aus den Fällen abgeleitet. Wiederverwendung verlangt einen passenden Gegenstand und Schutzvertrag; bloß ähnliche Wörter genügen nicht.
5. Erst danach ordnet der Koordinator die Kandidaten begrenzten Modulverantwortungen zu. Gleiche Modulverantwortung bedeutet weder identische Residues noch automatisch einen eigenen Prozess.
6. Der skeptische Gegenreview prüft Grenzfälle, unbelegte Erhaltung, Verzweigungen, sinnvolle Wiederverwendung und die Modulzusammensetzung. Korrekturen werden ausdrücklich dokumentiert, nicht durch heimliches Umschreiben der unabhängigen Abgaben.
7. Tabellen und lesbare Dokumente werden deterministisch aus diesen Eingaben erzeugt und auf Vollständigkeit, Referenzen und Reproduzierbarkeit geprüft.
8. Der vollständige Gegenreview fand neun konkrete Auflagen. Der Koordinator korrigiert sie durch hashgebundene Nachträge und getrennte Betriebs-/Teilnehmerverträge, ohne die unabhängigen Originale zu ändern. Ein weiterer kurzer Review prüft diese Korrekturen gesondert. `konflikt` ergänzt dabei die ursprünglichen Zweigarten für doppelte Schreibautorität ohne belegte Eskalation; KOR001 trennt verfügbares Zustellbudget vom vergangenen Versuch.

Das [verbindliche Abgabeprotokoll](../../../reviews/A7-PROTOCOL.md) enthält die Datenfelder und Qualitätsregeln. Agenten-, Run- und Workspacebezüge stehen im [Manifest](review-manifest.json).

## Was eine Zuordnung bedeutet

- `bedingt`: Kandidat nutzbar unter den ausdrücklich genannten Voraussetzungen.
- `teilweise`: nur benannte Teilfähigkeit bleibt erhalten; keine vollständige Wiederherstellung behauptet.
- `keines`: für den betrachteten Gegenstand bleibt unter dieser Annahme keine brauchbare Restfähigkeit; keine imaginäre Ersatzkopie oder allmächtige Instanz.
- `ungeklaert`: notwendige Erhaltung oder Voraussetzung ist nicht begründbar. Die konkrete fehlende Information wird benannt.

Das ist eine Zuordnung auch dann, wenn sie ehrlich negativ ausfällt. Totalverlust lässt sich nicht durch ein zusätzliches Kästchen im Diagramm beheben.

## Aussagegrenzen

- Die Stressoren sind kuratiert; sie bilden keine gemessene Wahrscheinlichkeitsverteilung.
- Verschiedene Kontexte mit derselben Modellfamilie sind keine voneinander unabhängigen menschlichen Expertisen.
- Zweigzahl, Kandidatenzahl und Modulzahl messen keine bewiesene Resilienz.
- Frühere `unresolved`-Fälle erhalten, wo möglich, bedingte Alternativen; das ist kein empirisches Schließen der Wissenslücke.
- Eine Prüfidee oder eine CSV-Referenz beweist weder den Verlauf noch die Erhaltung eines Gegenstands.
- Der Entwurf hält einen mutierenden Daemon, kanonische Ereignisse, wirkungsfreies Replay, begrenzte Plugin-Verträge und die v1-Grenze ohne Message-Store ein. Größere Einsatzgrenzen und Vertragsänderungen brauchen eigene Ratifikation.
- Es gibt keine Livefehler, realen Alarme, externen Geschäftsaktionen, Implementierungsänderungen oder Worktree-Eingriffe in diesem Auftrag.

Koordinator-Run: `3f3815e1-38d6-445e-97d3-bbebe92751f3`.
