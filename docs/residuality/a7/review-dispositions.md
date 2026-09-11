# A7 — Gegenreview und ausdrücklich umgesetzte Korrekturen

## Stand

Der [unabhängige Gegenreview](../../../reviews/skeptic/a7/review.md) hat alle vier Abgaben einschließlich aller Felder der 350 Fälle und 852 ursprünglichen Zweige gelesen. Das Inventar wurde als vollständig und überwiegend konkret begründet beurteilt, aber **neun konkrete Auflagen** wurden benannt. Eine pauschale semantische Gesamtfreigabe wurde ausdrücklich nicht erteilt.

Die folgenden Änderungen sind die Antwort des Koordinators. Der abgeschlossene [unabhängige Nachreview](../../../reviews/skeptic/a7/recheck.md) stuft **alle neun konkreten Dokumentauflagen als behoben** ein; die [Einzeldispositionen](../../../reviews/skeptic/a7/recheck.csv) bewahren die verbleibenden Grenzen. Das ist keine erneute semantische Gesamtfreigabe aller Fälle. Es handelt sich weiterhin um bedingte Entwurfsverträge, nicht nachgewiesene Produktfähigkeiten.

Die [Originalabgaben bleiben bytegenau eingefroren](discovery-freeze.json). Die [Nachträge](review-adjustments.json) binden sich an den Hash dieses Freeze und die bisherigen Zweig-IDs. Geänderte Zweige sind im lesbaren Katalog als Koordinatornachtrag markiert. Neue Zweige werden hinten ergänzt, nie zwischen bestehende IDs geschoben.

**Ergebnis des Koordinatornachtrags:** 350 Fälle, 855 Zweige und 153 Kandidaten. Davon stammen 852 Zweige und 152 Kandidaten aus den unabhängigen Abgaben. Neu sind drei explizite Alternativen und KOR001. Diese Zahlen sind keine globale Attraktor- oder Wirksamkeitszählung.

<a id="a7s01"></a>
## A7S01 — Noch kein richtiges Publikationspaket vorhanden

**Übernommen und korrigiert.** S156.B01 zählt nicht mehr GVR036. Unter seinen engeren ausdrücklichen Bedingungen bleibt nur die tatsächlich erhaltene gesperrte Publikationsabsicht GVR005 mit Prüfbedarf nutzbar. Das richtige Paket muss noch nicht existieren.

S156.B03 ist der neue getrennte Fall: Ein richtiges, inhaltlich geprüftes und rechtmäßig erhaltenes Projektpaket existiert tatsächlich. Nur dort wird GVR036 zugeordnet. M07 hält Inhalte, M15 begrenzt den zulässigen Umfang und M02 prüft eine davon getrennte Sendefreigabe. Ein richtiges Paket ist keine Veröffentlichungserlaubnis.

**Gegenprüfung:** falsches Rootpaket vorhanden, korrektes Paket noch nie erzeugt. Der gehaltene Zweig darf trotzdem untersuchbar sein, aber kein sauberes Paket behaupten.

<a id="a7s02"></a>
## A7S02 — Unverbrauchte Erlaubnis ist kein vergangener Versuch

**Übernommen und getrennt.** [KOR001](additional-residues.csv) bezeichnet den tatsächlich verfügbaren Rest einer gültigen Zustellrevision. S074.B02 verwendet diesen Kandidaten und den lesbaren Task, aber nicht OPR002.

S074.B03 setzt ausdrücklich einen erhaltenen früheren Versuch **und** eine neue zusätzliche Autorisierungsrevision voraus. OPR002 erhält nur die alte Versuchsevidenz; KOR001 den neuen Rest. Gemeinsame atomare Speicherung ist erlaubt, Gleichsetzung der Gegenstände nicht. Keine der beiden Angaben klärt frühere Außenwirkung oder Workspacebesitz.

<a id="a7s03"></a>
## A7S03 — Subscriber-Cursor und Runtimebeobachtung

**Übernommen und passend wiederverwendet.** S054.B01/B02 verwenden den geprüften Vergleichskandidaten OBR029 für autorisierten Cursor und begrenzten Ereignisbereich statt OPR014 für Runtime-Sessionbindung. Die jeweiligen zusätzlichen Ressourcen- und Eingabebelege bleiben erhalten.

Dies ist eine begründete Wiederverwendung zwischen den unabhängigen Bereichen, keine automatische Verschmelzung ihrer Residue-IDs. Gültiger Clientcursor ohne Runtimebeobachtung ist die notwendige Gegenprobe.

<a id="a7s04"></a>
## A7S04 — Wachstum und bloßer Schreibkonflikt

**Übernommen und umtypisiert.** S130.B02/S196.B02 sind nun `eskalation`: Fortgesetzte Erzeugung über Abbau ohne begrenzte Wiederkehr liefert keinen belegten Attraktor.

S235.B02/S245.B02 sind nun `konflikt`: fortbestehende doppelte Schreibautorität ohne zusätzlich begründetes Wachstum. Diese zusätzliche Zustandsart verhindert, dass aus zwei Schreibern automatisch eine Eskalation oder ein von außen erzwungener Lastzyklus wird. Die konkret lesbaren Restbestände bleiben nur unter ihren bisherigen Bedingungen zugeordnet.

<a id="a7s05"></a>
## A7S05 — Unabhängig erkennen und tatsächlich eingreifen

**Übernommen und als getrennte Teilverträge konkretisiert.** BDR037 wird als zusammengesetzter Vertrag von der unabhängig legitimierten Recoveryrolle M13 verantwortet, nicht allein von M12.

- M12: begrenzter unabhängiger Vergleich, ohne Schreib- oder Reparaturrecht.
- M13: anerkannte und erreichbare externe Eingriffsrolle außerhalb des kompromittierten Kerns.
- M14: tatsächlich wirksamer lokaler OS-/Verwahrentzug durch getrennte befugte Administration.
- M16: bei externen Wirkungen zusätzlich wirksamer Entzug am benannten Teilnehmer.

[Teilverträge](contract-facets.csv) und [Betriebsmodi O04–O07](operation-boundaries.csv) nennen Ausfallverhalten und Nachweise. Fehlt Eingriffsgewalt, zeigt der neue S350.B05 nur OBR038 als Beobachtungsrest. Der Zeuge erhält keine Shell; ein kompromittierter Factory-Kern muss den externen Entzug nicht selbst erlauben.

<a id="a7s06"></a>
## A7S06 — Lokale Sicherheitsreaktion ohne Factory

**Übernommen und nach Betriebsmodus abgegrenzt.** GVR035, BDR026 und BDR027 haben für die autonome Sicherheitsreaktion keine benötigte laufende Factory-Mitwirkung. Ihr eigener qualifizierter Controller, lokale Energie, Sensorik und Aktuatorgewalt bleiben echte Voraussetzungen.

Die M16→M14-Aufrufkante gilt ausdrücklich nur für Factory-vermittelte Adapteraufrufe. [O01–O03](operation-boundaries.csv) trennen vorgelagerten Auftrag, autonome lokale Reaktion und spätere Belegübernahme. Keine laufende M01-/M02-/M06-/M14-Instanz darf den autonomen Sicherheitsweg erst ermöglichen müssen. Das ist keine Erweiterung Factorys um einen Sicherheitskernel.

<a id="a7s07"></a>
## A7S07 — Eine echte Quelle für späteren Widerruf

**Übernommen und konkretisiert.** Der stärkere GVR011-Vertrag benötigt eine ausdrücklich konfigurierte, unabhängig verwaltete `AuthoritySource`. Die legitimierte Widerrufsstelle verantwortet den maßgeblichen Stand außerhalb rückgesetzter Factory-Stores. M16 vermittelt die authentische frische Abfrage, M02 bindet sie an Principal, Scope, Grantrevision und konkrete Handlung. M01 bleibt historisch.

Vertrauensanker und enges Abfragerecht dürfen nicht vom gerade strittigen alten Grant abhängen. Sonst wäre die Abfrage selbst blockiert; das wird nicht durch eine alte positive Antwort übergangen. Wirkliche Gültigkeit bis zur entfernten Annahme verlangt zusätzlich Teilnehmerprüfung.

S127.B01 setzt nun tatsächlich verfügbaren aktuellen Stand voraus. S127.B03 beschreibt den fehlenden Gegenbeleg: Ein konkret erhaltener Auftrag bleibt gehalten, aber GVR011 wird nicht behauptet. Die Quelle ist kein zweiter Taskstore und wird durch diese Dokumente nicht eingerichtet. Betreiber, Kosten, Zugriff und Vertrauensgrenze brauchen Ratifikation.

<a id="a7s08"></a>
## A7S08 — Stabile Reviewreferenzen erzwingen

**Übernommen und im Compiler erzwungen.** Vor dem Einlesen vergleicht `build.py` alle zwölf unabhängigen Abgaben mit dem eingefrorenen Hashbestand. Der Nachtrag selbst enthält dessen erwarteten SHA-256 und prüft zusätzlich ursprüngliche Feldwerte an den bearbeiteten Zweig-IDs. Ergänzungen sind nur hinter der aufgezeichneten ursprünglichen Zweigzahl erlaubt.

Negativprüfungen verändern eine isolierte Kopie einer eingefrorenen Datei und vertauschen die S156-Zweige im Speicher. Beide müssen fehlschlagen, statt eine bestehende Reviewreferenz umzudeuten. Die Originale bleiben unverändert. Der Freeze ist eine nachvollziehbare Revisionsbindung, kein Schutz gegen einen Angreifer, der sämtliche Prüfcodes und Vertrauensanker gemeinsam ändern darf.

<a id="a7s09"></a>
## A7S09 — Ein vollständiger Prüfaufruf

**Übernommen.** `build.py` liest und prüft nun auch die Abhängigkeitstabelle einschließlich Bootstrapzyklen. Entscheidende Architektur- und Korrektureingaben werden mitgehasht. Eine synthetische Rückkante M07→M01 muss bereits den Compiler ablehnen lassen.

Der vollständige Nurlese-Aufruf lautet:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a7/verify.py
```

Er prüft Quellenfreeze, Nachträge, explizite Bindings, Abhängigkeiten, erzeugte Dateien und Dokumenttests gemeinsam. Das ist weiterhin **kein Nachweis tatsächlicher Ressourcenisolation, menschlicher Befugnis, Teilnehmergarantien, physischer Sicherheit oder empirischer Attraktion**.
