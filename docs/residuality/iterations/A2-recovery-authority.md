# A2 — Recovery ist eine neue Gegenwart, keine Fortsetzung des Snapshots

Input: A1-Lücken plus S161–S180 (Organisation, Wachstum, neue Betriebsarten). Korpus jetzt 180 Karten und 387 direkte Inzidenzen. Erweiterungen C02, C06, C08, C10, C14, C16, C18, C19, C21, C26.

## Strukturänderung gegenüber A1

```text
Instanzinkarnation + Core-Bootstrap
  → ein Writer / lesbarer Recovery-Modus
  → unabhängiger lokaler Kontrollpfad vor optionalen Plugins
  → rekonstruierte Historie != aktuelle Handlungsbefugnis
  → aktuelle Identitäts-, Daten- und Freigabepreconditions
  → erst dann begrenzter Dispatch

Restore-Set
  = konsistenter Store + notwendige Referenzinhalte + Format/Versionsmanifest
    + Konfigurationsrevisionen + extern erreichbarer Recovery-Prozess
  ≠ Secretwerte im normalen Backup
```

## Matrixgetriebene Entscheidungen

| Entscheidung | Auslöser / gemeinsame Residues | Revidierter Vertrag | Kosten und Alternative |
|---|---|---|---|
| A2-D1: Recovery-Epoche | S015/S065/S127, R01/R02/R03/R09 | Restore erzeugt eine neue dokumentierte Inkarnation. Alle historischen Live-/Auth-Aussagen werden zunächst als unbestätigt behandelt. Queued ohne Versuch bedeutet nur „im Snapshot kein Versuch“. Dispatch bleibt bis Evidenzprüfung gesperrt. | Mehr manuelle Klärung statt falscher Gewissheit aus Snapshot-Leere. |
| A2-D2: lokale und externe Exklusivität unterscheiden | S065/S094/S173/S175 | Lokaler Lock bleibt. Für mögliche Klone braucht der Teilnehmer eine monotone Generation/Precondition oder es gilt manueller exklusiver Single-active-Betrieb. Eine zufällige neue UUID allein fencet keinen alten Writer. | Kein automatisches Failover ohne echten Fencing-Beleg. Kein verteilter Konsenscluster in v1. |
| A2-D3: Kontrolle vor Komfort | S011/S045/S055/S091/S180 | Minimaler Bootstrap lädt keine optionalen Plugins. Diagnose und Aufnahme-/Dispatch-Sperre benötigen keinen freien Worker. Speicher- und Hostbudgets reservieren Kontrollkapazität. | Eigenständiger getesteter Bootpfad, nicht zweiter Mutationseigentümer. |
| A2-D4: historisch gepinnt, aktuell erlaubt | S027/S064/S083/S111/S163 | Historische Intention bleibt immutable. Dispatch prüft heutige Berechtigung, exakte Payload-/Empfängerbindung, Verfall, Teilnehmerpreconditions und Widerruf. | Veralteter Run kann trotz vollständiger Historie nicht fortgesetzt werden. |
| A2-D5: vollständige Evidence Closure | S006/S014/S016/S070/S178 | Recoverymanifest zählt nicht nur DB-Dateien, sondern alle zur Interpretation erforderlichen Inhalte und Versionen. Unbekannte Importvergangenheit bleibt markiert. | Aufwand und Speicher; Content-Hash ist Integritätsnachweis, kein Ersatz für Inhalt. |
| A2-D6: menschliche Kapazität ist begrenzt | S091–S100/S161/S168 | Entscheidungsqueue mit Alter, Kontext, Optionen und benannter legitimierter Vertretung. Globale Admission berücksichtigt Blockierung und erhält Diagnosekapazität. | Keine automatische Zustimmung nach Frist und keine heimliche Machtübertragung. |
| A2-D7: Zeitquellen trennen | S023/S026–S029/S148 | Monotone lokale Frist, Schedule-Geschäftszeit, Beobachtungsalter und Freigabegültigkeit sind verschiedene Felder/Verträge. | Keine einzige allmächtige Timestamp-Spalte. |

## Widerruf und Point of no return

Eine neue Aktion wird direkt vor Dispatch erneut geprüft. Das entfernt nicht das Fenster zwischen lokaler Prüfung und externer Annahme. Wo der Teilnehmer Version/Expiry atomar bei Annahme prüft, wird diese Bedingung propagiert. Sonst ist der mögliche Annahmezeitraum als Unsicherheit zu führen. Nach Annahme bedeutet Widerruf: keine weiteren Aktionen; nicht: die bereits angenommene Wirkung sei rückgängig.

## Matrixprüfung nach A2

Auf 180 Karten: A1 = **41/111/28**, A2 = **117/60/3** (vollständig/teilweise/keine zugeordneten Obligationen benannt). Das sind syntaktische Entwurfsantworten. R09 ist mit 41 direkten Karten der größte Knoten; das ist ein Review-Hotspot, kein mathematischer Beweis seiner Wichtigkeit. R08 und R03 haben wenige direkte Karten, sind aber gemeinsame Voraussetzungen vieler Restfähigkeiten und dürfen nicht wegoptimiert werden.

## Neue Grenzen, die A2 nicht auflöst

- S131/S132/S138: Löschung, Legal Hold und vollständiger Replay sind nicht gleichzeitig bedingungslos garantierbar.
- S098/S104/S110/S167: unabhängiger Prozess oder grünes Gate ist kein unfehlbarer Erkenntnismechanismus.
- S161/S162/S165: eine benannte Person kann verschwinden, befangen sein oder mit anderer legitimer Autorität streiten.
- S117/S121/S172/S177: Same-User-Policy ist kein Schutz gegen feindliche Repositories, Plugins oder Root-Administratoren.
- S149/S155/S179: Systeme müssen auch lesbar stillgelegt werden können, nicht nur wieder starten.

**Nächste Runde:** S181–S200 greifen physische Totalverluste, widersprüchliche Gesellschaftsannahmen und kombinierte Failure-Pfade an. A3 ergänzt nicht automatisch mehr Infrastruktur, sondern explizite Verlust-, Einsatz-, Nachfolge- und Ausstiegsgrenzen.
