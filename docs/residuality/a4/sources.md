# Quellen und Suchverfahren für A4

## Was tatsächlich recherchiert wurde

Am **2026-09-09** wurden die folgenden öffentlichen Webseiten per HTTPS abgerufen und die einschlägigen Textpassagen ausgewertet. Es wurden keine Konten angelegt, keine Dienste eingerichtet und keine Benachrichtigungen versandt.

| Kennung | Quelle / gelesener Umfang | Übernommener Denkanstoß | Nicht daraus ableiten |
|---|---|---|---|
| B01 | [ISO25000.com: ISO/IEC 25010 overview](https://iso25000.com/index.php/en/iso-25000-standards/iso-25010), öffentliche Qualitätsmodellübersicht | Qualität umfasst verschiedene Stakeholderbedürfnisse, nicht nur Funktion: unter anderem Performance, Security und Maintainability. | Das ist eine erläuternde Webseite, nicht der vollständig gelesene normative ISO-Text. Keine Normkonformität oder vollständige Normabdeckung behauptet. |
| B02 | [Google SRE: Monitoring Distributed Systems](https://sre.google/sre-book/monitoring-distributed-systems/), insbesondere Black-/White-box, Golden Signals, Tail und Alarmbelastung | Latenz, Verkehr, Fehler und Sättigung beobachten. Symptome von Ursachen unterscheiden. Menschliche Aufmerksamkeit ist knapp. | Ein Hostsignal beweist keine fachliche Gesundheit. Googles Betriebsgröße ist kein Auftrag, Factory ebenso groß zu bauen. |
| B03 | [Google SRE: Addressing Cascading Failures](https://sre.google/sre-book/addressing-cascading-failures/), insbesondere Deadlines, Retrybudgets, Kaltstart und bimodale Latenz | Gesamtfristen weiterreichen, Wiederholungen begrenzen, kalte Zustände testen. Eine kleine hängende Teilpopulation kann ein System sättigen. | Ein Timeout beweist nicht das Ende einer bereits angenommenen Außenwirkung. Load Shedding darf keine kanonische Evidenz löschen. |
| B04 | [Dean / Barroso: The Tail at Scale](https://research.google/pubs/the-tail-at-scale/), Publikationsseite und Abstract | Seltene Verzögerungen können bei wachsender Breite den Gesamtauftrag dominieren. | Der Volltext wurde hier nicht ausgewertet. Keine konkreten Messwerte auf Factory übertragen. Insbesondere keine allgemeine Freigabe für spekulative doppelte Geschäftsaktionen. |
| B05 | [OWASP ASVS](https://owasp.org/www-project-application-security-verification-standard/), Projektübersicht und Hinweise zu Anforderungen und Versionsbindung | Security in konkrete überprüfbare Anforderungen übersetzen, nicht als pauschales Label behandeln. | Kein vollständiger ASVS-Audit und kein behauptetes Verifikationslevel. Die Agent-/Scope-Angriffe sind eigene Factory-Gegenproben, keine wörtlich übernommenen ASVS-Testfälle. |
| B06 | [Prometheus: Alerting](https://prometheus.io/docs/practices/alerting/), einschließlich Metamonitoring | Wenige handlungsrelevante Alarme, Ende-zu-Ende-Prüfung der Zustellkette und externe Black-box-Beobachtung des Monitorings. | Keine Produktauswahl zugunsten Prometheus. Ein zweiter Prozess auf derselben Maschine ist noch keine unabhängige Überwachung. |

Ein zusätzlicher Abruf des AWS-Artikels „Timeouts, retries, and backoff with jitter“ lieferte im verwendeten Textzugang keinen auswertbaren Artikeltext. Er wird deshalb **nicht** als gelesener Sachbeleg verwendet. Retry-/Deadline-Aussagen stützen sich hier auf B03.

Lokale Grundlagen: [A3](../architecture-A3.md), [bestehende Quellen und Codebefunde](../sources.md), [bestehende Obligationen](../controls.csv). Der Produktcode wurde in dieser Erweiterung nicht erneut vollständig auditiert. Vorhandene Implementierungsbefunde sind daher weiterhin zeitgebundene Befunde des früheren Arbeitsstands.

## Wie aus Qualitätszielen Stressoren wurden

Ein Qualitätswort allein ist keine Suchstrategie. **„Security“** ist ein Ziel. **„Einen berechtigten Vermittler gegen einen fremden Scope handeln lassen“** ist die Suchbewegung. **„Das Plugin verwendet ein Credential des falschen Scopes“** ist die konkrete Gegenprobe.

Für jede der [50 Strategien](generated/strategies.md):

1. Qualitätsziel und angegriffene Annahme nennen.
2. Drei konkrete Gegenproben formulieren — üblicher Mechanismus, Verschärfung oder ungewöhnliche Kombination; nicht immer dieselbe Dreierschablone.
3. Fragen, welche bestehende Restfähigkeit trägt und welche Regel dafür fehlt.
4. Die konkrete Regel einem Residue und einem vorgeschlagenen Experiment zuordnen.
5. Nicht lösbare Grenzen sichtbar erhalten.

**50 Strategien sind die vollständige Strategieliste dieses Durchgangs.** Die 20 alten Perspektiven waren eine andere Gliederung der ersten 200 Karten; sie werden nicht als weitere 20 Strategien hinzuaddiert. Neue Karten S201–S350 ergänzen die unveränderten S001–S200. Es gibt damit 350 Situationen, aber ausdrücklich nicht 350 unabhängige Fehlermechanismen. Wiederkehrende Mechanismen und absichtliche Verschärfungen helfen gerade beim Verdichten.

## Was „mutig in die Außenbereiche“ hier bedeutet

Wir variieren Verzögerungen nicht nur zwischen üblichen Millisekundenwerten, sondern bis zu Wochen, Jahrzehnten und **gar keiner Antwort**. Wir variieren außerdem, was in dieser Wartezeit geschieht: Rechte verfallen, Menschen wechseln, Inhalte müssen gelöscht werden, Anbieter vergessen ihre Operationen, der Rechner wird restauriert.

Beispiele: zwölf Jahre menschliche Wartezeit (S221), Ausführung erst 2046 (S232), Plattformwechsel 2080 (S341), prinzipiell unbeschränkte Teilnehmerlatenz bei endlicher Erinnerung (S347). Das sind bewusst gewählte Randbedingungen, keine Vorhersagen.

Weder „p99.999“ noch ein sehr großer Zahlenwert benennt ohne Daten eine reale Wahrscheinlichkeit. Es wurden **keine Latenzverteilungen gemessen**, keine zufälligen Stichproben gezogen und keine unabhängigen Experteninterviews durchgeführt. Die Strategien und ihre drei Karten wurden vom gleichen Analysten kuratiert. Auch die Kaskaden sind gedankliche Gegenproben, kein separater Holdout.

## Abbruch- und Auswertungsregel

Der Durchgang endet beim vereinbarten Umfang von 50 Suchstrategien mit drei Karten, **nicht** bei bewiesener Konvergenz. Die [gemeinsame Matrix](generated/incidence-matrix.csv) zeigt direkte vertragliche Zuordnung, keine Eintrittswahrscheinlichkeit und keinen bestandenen Test. Bestehende C33–C36 bleiben offen. Vollständig benannte Grenzen können dennoch vollständigen Verlust bedeuten, etwa S345.

Die [Architekturableitung](architecture-A4.md) enthält die eigentliche qualitative Schlussfolgerung. Die Zahlen in [summary.md](generated/summary.md) sind nur eine Konsistenz- und Navigationshilfe. Zusätzliche Regeln verbessern die syntaktische Bewertung konstruktionsbedingt; diese Zahl ist kein unabhängiger Nachweis für bessere Architektur.
