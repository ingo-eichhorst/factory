<!-- Generated exclusively by docs/residuality/build_matrices.py. -->
# Stressor-Katalog

Lesesicht von `../stressors.csv`; ausschließlich hypothetische Szenarien.

## P01 — Physik Gerät Energie Umwelt

Was bleibt ohne ursprüngliches Gerät und konstante Ressourcen

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S001 | Netzstrom fällt während Commit und Prompt-Übergabe aus | Strom reicht bis zur Bestätigung | Letzter dauerhafter Versuch bleibt prüfbar | R01 R02 | halten: Nicht bestätigter Effekt braucht Rekonziliation |
| S002 | Akku bläht sich auf und Gerät muss sofort außer Betrieb | Originalhardware bleibt verfügbar | Arbeit aus unabhängigem Recovery-Set lesbar | R04 R11 | halten: Stillstand bis Ersatzhardware verfügbar |
| S003 | Thermal Throttling macht alle Deadline-Schätzungen falsch | Rechenkapazität ist konstant | Begrenzte Teilmenge und Kontrollpfad bleiben bedienbar | R07 R13 | degradiert: Keine garantierte ursprüngliche Lieferzeit |
| S004 | RAM-Fehler korrumpiert ein Ergebnis ohne Prozesscrash | Lebender Prozess liefert unbeschädigte Bedeutung | Artefakt und unabhängige Prüfung bleiben getrennt | R04 R14 | halten: Gemeinsame Korruption kann beide Prüfungen treffen |
| S005 | Einbruch entwendet den entsperrten einzigen Mac | Physische Kontrolle ist dauerhaft | Unabhängige Kopien erlauben späteren Wiederaufbau | R09 R11 R15 | verlust: Bereits zugängliche Daten gelten möglicherweise als kompromittiert |
| S006 | Brand vernichtet Gerät und daneben gelagerte Backupplatte | Zwei Datenträger sind zwei Ausfalldomänen | Nur räumlich unabhängiges Recovery-Set überlebt | R11 | verlust: Alles seit letzter unabhängiger Kopie kann fehlen |
| S007 | Externe SSD trennt sich bei Scope-Kanonisierung | Ein Pfad bleibt während eines Übergangs derselbe | Task bleibt wartend statt an Ersatzpfad gestartet | R01 R05 | halten: Filesystem-TOCTOU muss separat getestet werden |
| S008 | macOS startet mitten in einer Freigabe ungefragt neu | Menschlicher Klick und Wirkung bilden eine atomare Einheit | Commitgrenze und Freigabeevidenz bleiben nachvollziehbar | R02 R09 | halten: Nicht committete menschliche Intention ist nicht beweisbar |
| S009 | Hardware ersetzt Apple Silicon durch inkompatible Plattform | Runtime ist portabel weil Tasks portabel sind | Arbeitspaket bleibt exportierbar ohne lokalen Executor | R05 R16 | halten: Neuer Plattformadapter kann ein eigenes Projekt benötigen |
| S010 | Wohnort verliert eine Woche Strom und Internet | Lokal bedeutet stets verfügbar | Unabhängig rekonstruierbare Aufträge warten | R10 R11 R13 | halten: Geschäftsfristen können trotzdem verstreichen |

## P02 — Storage Datenhistorie Integrität

Welche Bedeutung ist ohne heutige Projektionen rekonstruierbar

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S011 | SSD ist vor dem Intent-Commit voll | Audit kann immer noch geschrieben werden | Keine neue Wirkung ohne durable Absicht | R02 R07 | halten: Diagnose benötigt reservierte Ressourcen |
| S012 | Bitrot beschädigt einen alten kanonischen Event | Append-only bedeutet unveränderlich korrekte Bytes | Integritätsfehler wird an konkreter Position sichtbar | R04 R11 | halten: Kein Überspringen mit vorgetäuschtem vollständigem Replay |
| S013 | Projektionsmigration endet nach halber Tabellenänderung | Schemaänderung ist automatisch wiederanlaufbar | Transaktion und bekannte Schemaidentität begrenzen Schaden | R04 R16 | halten: Migrationsatomizität muss real nachgewiesen werden |
| S014 | WAL-Datei wird beim Dateikopieren vergessen | Eine kopierte sqlite-Datei ist ein konsistentes Backup | Geprüfter konsistenter Snapshot bleibt Wiederaufbauquelle | R04 R11 | verlust: Fehlende committed WAL-Fakten sind nicht herleitbar |
| S015 | Restore stellt eine Woche alte Tasks ohne spätere Versuche her | Kein Journalversuch bedeutet nie ausgeführt | Snapshot bleibt historische Evidenz mit Dispatch-Sperre | R01 R02 R09 | halten: Teilnehmer ohne Abfrage erfordern menschliche Auflösung |
| S016 | Referenziertes Artefakt wurde manuell gelöscht | Ein Content-Hash ersetzt den Inhalt | Fehlende Evidenz wird explizit markiert | R04 R11 | verlust: Ohne unabhängige Kopie bleibt Inhalt verloren |
| S017 | Backup-Key ist verloren obwohl alle Backups intakt sind | Vorhandene verschlüsselte Bytes sind wiederherstellbare Daten | Nur separat erschließbare Schlüssel erlauben Recovery | R11 | verlust: Ohne Schlüssel kein Versprechen auf Rekonstruktion |
| S018 | Reduktor liest heutige Registry beim historischen Replay | Determinismus folgt aus gleicher Eventreihenfolge allein | Versionierte gespeicherte Inputs erlauben pure Rekonstruktion | R04 R05 | halten: Alte nicht aufgezeichnete Inputs brauchen Importgrenze |
| S019 | Schema enthält neueren Payload-Typ als installiertes Binary | Jede bekannte Datei kann jedes Binary lesen | Read-only Diagnose nennt genaue Inkompatibilität | R04 R08 R16 | halten: Kein automatischer Downgrade oder stilles Überspringen |
| S020 | Backup-Retention löscht die einzige Kopie benötigter Inhalte | Alte Daten sind nach Alter allein entbehrlich | Referenzinventar zeigt notwendige Recovery-Menge | R04 R11 R12 | halten: Retention und vollständiger Replay können unvereinbar sein |

## P03 — Zeit und Gültigkeit

Welche Zeit meint ein Termin eine Frist oder eine Erlaubnis

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S021 | Sommerzeit wiederholt dieselbe lokale Cron-Minute | Lokales Minutenlabel ist eindeutige Occurrence | Explizit gewählte DST-Semantik erhält Triggeridentität | R13 | degradiert: Ein- oder zweimalige Ausführung muss fachlich entschieden sein |
| S022 | Sommerzeit überspringt geplanten Zeitpunkt | Jede lokale Uhrzeit existiert | Auslassen oder Nachholen ist sichtbar spezifiziert | R13 | degradiert: Kein implizites Nachholen irreversibler Aktionen |
| S023 | NTP setzt Uhr zwölf Stunden zurück | Wallclock ist monoton | Lokale Timeouts laufen monoton und Schedule-Zeit bleibt gesondert | R13 | degradiert: Externe Zeitangaben können weiter unzuverlässig sein |
| S024 | Laptop erwacht nach sechs Wochen mit tausenden fälligen Runs | Nachholen ist immer erwünscht | Begrenzte Catch-up-Queue bleibt inspizierbar | R07 R13 | degradiert: Verfallene Arbeit darf nicht ungeprüft ausgeführt werden |
| S025 | Zeitzonenregeln ändern sich nach Schedule-Erstellung | IANA-Zone hat unveränderliche historische und künftige Bedeutung | Regelrevision und Occurrence-Entscheidung bleiben nachvollziehbar | R05 R13 | halten: Betroffene künftige Termine brauchen Review |
| S026 | Zwei Teilnehmer bestätigen Wirkungen mit widersprüchlichen Uhrzeiten | Zeitstempel liefern globale Kausalordnung | Operation- und Kausalidentitäten bleiben ordnende Evidenz | R02 R04 R13 | degradiert: Keine erfundene globale Echtzeitordnung |
| S027 | Freigabe läuft während langem Plugin-Start ab | Gültigkeit bei Queueing reicht bis Dispatch | Aktuelle Freigabeprecondition stoppt neue Wirkung | R09 R13 | halten: Bereits angenommene Wirkung bleibt möglich |
| S028 | Ein Task verarbeitet einen inzwischen abgelaufenen Preis | Gepinnte Inputs sind auch heute fachlich gültig | Alte Intention bleibt sichtbar und wird neu bewertet | R05 R13 | halten: Fachliche Gültigkeitsregel muss vom Domänenverantwortlichen kommen |
| S029 | Maschine geht während monotonic-basierter Frist in Sleep | Monotone Uhr misst überall denselben Sleep-Anteil | Zeitquellenvertrag macht Fristinterpretation explizit | R13 | halten: OS-spezifische Uhrsemantik braucht Tests |
| S030 | Cron-Plugin feuert denselben Trigger nach jedem Neustart erneut | Ein Transportaufruf entspricht einem neuen Termin | Durable Occurrence-Deduplikation verhindert neue Runidentität | R02 R13 | degradiert: Deduplikation muss denselben fachlichen Trigger erkennen |

## P04 — Runtime Prozesse Workspace

Wann darf aus Beobachtung Identität oder Ende folgen

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S031 | Herdr ist unerreichbar während der Harness weiter Dateien schreibt | Nicht beobachtbar heißt beendet | Lease schützt den möglicherweise belegten Workspace | R01 R08 | halten: Freigabe braucht Endevidenz statt Timeout |
| S032 | Pane-ID wird nach Neustart einer anderen Session gegeben | Paneadresse ist dauerhafte Identität | Unbekannte Zuordnung bleibt blockiert | R01 | halten: Ohne stabile Korrelation keine automatische Adoption |
| S033 | Transcript-Dateiname verliert die erwartete UUID | Parserfehler darf als Identitätsmatch gelten | Arbeitsauftrag bleibt ohne Session-Zuordnung lesbar | R01 R05 | halten: Adapter braucht alternative belastbare Identität |
| S034 | Hook fehlt aber Screen-Parser meldet idle | Heuristische und autoritative Beobachtung sind austauschbar | Nur manuelle Bestätigung kann Fortschritt legitimieren | R01 R10 | halten: Kein automatischer Fallback auf schwächere Evidenz |
| S035 | Harness meldet done nach einem Zwischenturn | Turnende ist Taskabschluss | Ergebnisstatus bleibt an tatsächliche Abgabe gebunden | R01 R14 | degradiert: Worker kann weiterhin fachlich irren |
| S036 | Alte autoritative Beobachtung trifft nach neuer Session ein | Autorität impliziert Aktualität | Generation und Beobachtungsalter verhindern falsche Promotion | R01 R13 | halten: Alter muss anhand passender Uhr und Generation beurteilt werden |
| S037 | Zwei Case-Varianten desselben Workspace werden parallel reserviert | Unterschiedliche Pfadstrings sind unterschiedliche Verzeichnisse | Kanonische aktuelle Identität erhält Exklusivität | R01 R03 | halten: Filesystem-Aliasing muss über zeitliche Renames getestet werden |
| S038 | Ein untergeordneter Prozess überlebt den gestoppten Harness | Parent-Ende beweist Schreibende aller Nachkommen | Lease bleibt bis geklärter Ressourcenverantwortung gehalten | R01 R06 | halten: Prozessgruppenbeobachtung ist keine harte Sandbox |
| S039 | Temporärer Worker wartet tagelang auf Permission | Temporär bedeutet kurze Ressourcenbelegung | Task und Lease bleiben sicher wartend | R07 R10 | halten: Budgetmangel darf keine Freigabe erzwingen |
| S040 | Herdr-Restart wird fälschlich als kompletter Maschinenneustart klassifiziert | Restart-Klasse ist eine sichere Todesbescheinigung | Explizite Evidenz schlägt Vermutung im Recoverypfad | R01 | halten: Allein der Funktionsname darf keine Lease lösen |

## P05 — Plugin-Protokoll und Supervision

Wie lokal bleibt ein Ausfall der Integrationsgrenze

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S041 | Plugin schreibt Debugtext auf stdout zwischen Protokollframes | Prozessgrenze garantiert parsebare Daten | Aktivierung wird isoliert quarantänisiert | R06 | degradiert: Andere Aktivierungen benötigen eigene Ressourcenbudgets |
| S042 | Plugin meldet einen Frame von mehreren Terabytes an | Längenheader sind harmlos bis Body empfangen wird | Host verweigert vor Allokation und hält Kontrollpfad frei | R06 R07 | degradiert: Parsergrenzen müssen vor Allokation greifen |
| S043 | Plugin beantwortet Healthchecks aber blockiert jede Geschäftsoperation | Healthy bedeutet fachlich funktionsfähig | Operation-Deadlines grenzen betroffene Arbeit ein | R02 R06 | halten: Unklarer Effekt darf nicht erneut gesendet werden |
| S044 | Plugin crasht sofort nach jedem automatischen Restart | Neustart heilt jeden Fehler | Restartbudget endet in sichtbarer Quarantäne | R06 | degradiert: Expliziter Operatorretry kann denselben Fehler wiederholen |
| S045 | Optionales Pluginmanifest ist syntaktisch kaputt | Alle Konfiguration muss vor lokaler Diagnose gültig sein | Minimaler Bootstrap zeigt Fehler ohne Pluginaktivierung | R06 R08 | degradiert: Core-Bootstrap selbst braucht verifizierbare Minimaldaten |
| S046 | Plugin liefert Erfolg vor dauerhaftem Commit beim Teilnehmer | Erfolgsmeldung ist dauerhafter Wirkungsbeleg | Unabhängige Rekonziliation hält Ergebnis ehrlich | R02 R14 | halten: Unehrlicher Teilnehmer kann Bestätigung fälschen |
| S047 | Plugin-Unterprozess verbraucht alle File Descriptors | Nur Hauptprozessverbrauch zählt | Host-Budgets und Kontrollreserve begrenzen Ausfallradius | R06 R07 | degradiert: OS-weite Erschöpfung kann trotzdem Diagnose zerstören |
| S048 | Kompatibles Major-Protokoll ändert die Bedeutung eines optionalen Felds | Schema-Kompatibilität garantiert Semantik | Konformitätsfixtures verweigern unbewiesene Aktivierung | R06 R16 | halten: Nicht spezifizierte Semantik bleibt ein Vertragsproblem |
| S049 | Ein Scope deaktiviert Plugin während dessen Effekt in flight ist | Deaktivierung annulliert Außenwirkung | Durable Operation bleibt nach Deaktivierung rekonzilierbar | R02 R09 | halten: Stoppen kann bereits angenommenen Effekt nicht ungeschehen machen |
| S050 | Plugin behauptet reversibel aber inverse Operation verliert Daten | Manifestklassifikation ist ausreichender Beweis | Verifizierter Kompensationsvertrag begrenzt automatische Versprechen | R02 R06 R14 | halten: Nicht invertierbare Wirkung muss als irreversibel behandelt werden |

## P06 — Netzwerk und entfernte Beobachter

Welche Fakten überleben verlorene oder veraltete Antworten

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S051 | Netz partitioniert nach externer Annahme vor Response | Timeout beweist Nichterfolg | Dauerhafte Operation bleibt unknown ohne blinden Retry | R02 R09 | halten: Rekonziliation braucht Teilnehmerunterstützung |
| S052 | Provider DNS zeigt auf falschen Endpunkt | Bekannter Hostname identifiziert den Geschäftspartner | Endpoint- und Empfängerbindung verhindert stillen Zielwechsel | R06 R09 | halten: Transportauthentisierung bleibt Pluginpflicht |
| S053 | TLS-Zertifikat läuft ab während langer Offlinephase | Offline erzeugte Freigabe genügt bei Rückkehr | Kein neuer Dispatch ohne validierten Teilnehmer und gültige Erlaubnis | R09 R13 | halten: Zertifikatserneuerung ist externer Betreiberprozess |
| S054 | WebSocket-Client liest nie seine Events | Ein Subscriber kann unbegrenzt langsam sein | Cursorbasierte Wiederaufnahme erlaubt Disconnect statt Pufferwachstum | R04 R06 | degradiert: Client muss veraltete Projektion sichtbar machen |
| S055 | HTTP-Transport fällt aus während lokaler Betrieb intakt ist | Ein API-Ausfall bedeutet kompletten Factory-Ausfall | Bundled lokaler Kontrollpfad bleibt übrig | R06 R08 | degradiert: Remote-Nutzer braucht alternativen freigegebenen Zugang |
| S056 | Remote-Client zeigt tagelang gecachten Status running | Lesbares UI ist aktuelle Wahrheit | Cursor und Alter machen veraltete Sicht erkennbar | R01 R04 R13 | degradiert: Operator darf stale Anzeige nicht als Freigabegrund nutzen |
| S057 | Netzwerk-Antworten kommen doppelt und vertauscht | Transportreihenfolge ist Domänenreihenfolge | Operationidentität und Streamrevision begrenzen doppelte Mutation | R02 R04 | degradiert: Teilnehmer muss dieselbe Idempotenzsemantik respektieren |
| S058 | Ein nicht-lokaler Listener wird versehentlich öffentlich gebunden | Installierte Integration ist harmlose lokale Option | Explizite Aktivierung und Scope-Auth begrenzen routbare Fähigkeiten | R06 R09 R15 | grenze: Öffentliche Exposition braucht eigenständigen Sicherheitsreview |
| S059 | Tailscale und SSH fallen gemeinsam aus während Operator fern ist | Zwei Zugänge sind unabhängige Zugänge | Tasks bleiben ohne neue Freigaben erhalten | R08 R10 | halten: Lokaler Zugriff ist aus der Ferne nicht garantiert |
| S060 | Teilnehmer drosselt alle Anfragen über Tage mit Rate Limit | Ein Retry wird bald Erfolg haben | Begrenzte Wartequeue ohne Wiederholungssturm | R06 R07 R13 | degradiert: Geschäftstermin kann verfallen |

## P07 — Identität Topologie Berechtigung

Welche Identität bleibt unter Umzug Klonen und Delegation erhalten

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S061 | Registry benennt Zielagent um nachdem Task queued wurde | Empfänger kann bei Assignment neu geraten werden | Gepinnte Zielintention bleibt sichtbar | R05 | halten: Neuer Empfänger braucht bewusste Entscheidung |
| S062 | Scope-Verzeichnis wird gelöscht und am selben Pfad ersetzt | Gleicher Pfad bedeutet gleicher Scope-Inhalt | UUID und aktuelle Pfadidentität werden getrennt geprüft | R01 R05 | halten: Inode allein ist keine langfristige Geschäftsidentität |
| S063 | Symlink ändert Ziel zwischen Validierung und Dateischreibzugriff | Validierung schützt jeden späteren Zugriff automatisch | Unveränderte Taskintention erlaubt Abbruch bei Identitätsdrift | R01 R05 | halten: Deskriptorbasierte Pfadbindung ist noch Implementierungsarbeit |
| S064 | Parent-Scope verschiebt sich und ändert Delegationsrechte | Historische Hierarchie ist heutige Erlaubnis | Alte Kette bleibt Audit und neue Policy wird geprüft | R05 R09 | halten: Bereits gestartete Arbeit braucht eigene Widerrufsregel |
| S065 | Zwei Klone derselben Installation starten auf getrennten Macs | Lokaler flock ist globaler Single-Writer-Nachweis | Inkarnation und externe Preconditions begrenzen Doppelwirkung | R02 R03 | halten: Ohne gemeinsames Fencing bleibt nur manueller exklusiver Betrieb |
| S066 | Client zeigt legitime Session-ID eines anderen Agents vor | Bekannte UUID ist Authentisierung | Transportbindung bestimmt Actor statt Payloadangabe | R09 R15 | grenze: Same-User-Dateizugriff bleibt außerhalb harter Isolation |
| S067 | Task delegiert A nach B nach C nach A | Peer-Delegation terminiert ohne Historie | Persistierte Kette verweigert Wiederbetreten | R04 R09 | degradiert: Neue unabhängige Tasks können semantisch trotzdem Kreise bilden |
| S068 | Agent erzeugt statt Weiterdelegation immer neue identische Tasks | Azyklische Taskkette begrenzt globale Arbeit | Admissionbudget stoppt unbegrenzte autorisierte Taskproduktion | R07 R10 | degradiert: Semantische Duplikate lassen sich nicht perfekt erkennen |
| S069 | Scope wird stillgelegt während alte Tasks und Memory existieren | Entfernte Registryzeile darf History löschen | Retirement erhält Provenienz und sperrt neue Zustellung | R05 R12 R16 | halten: Retentionentscheidung benötigt Owner |
| S070 | Case-sensitive Export wird auf case-insensitive Dateisystem restauriert | Dateinamen bleiben überall unterscheidbar | Manifest meldet Konflikte vor Aktivierung | R04 R11 R16 | halten: Automatisches Umbenennen könnte Linkidentität zerstören |

## P08 — Task-Lifecycle und Executor

Welche Übergänge sind atomar und welche nur Absichten

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S071 | CLI wiederholt create nach verlorener Antwort | Neue API-Anfrage bedeutet neue Intention | Idempotenzidentität liefert dasselbe akzeptierte Ergebnis | R02 R04 | degradiert: Andere Payload mit gleichem Key muss Konflikt bleiben |
| S072 | Cancel trifft nach Zustellcommit aber vor Writer-Aufruf ein | Cancel kann laufende Wirkung atomar verhindern | Cancelabsicht und möglicher Dispatch bleiben getrennt | R02 R09 | halten: Definierter Point-of-no-return bleibt nötig |
| S073 | Zwei Assignments sehen denselben idle Worker | Lesender Idlecheck reserviert bereits Kapazität | Transaktionale Assignmententscheidung verhindert Doppelbelegung | R01 R03 | degradiert: Runtime kann sich außerhalb Kernel trotzdem ändern |
| S074 | Blocked Task wird requeued ohne neue Zustellautorisierung | Statuswechsel erzeugt legitimen Retry | Dauerhaftes Zustellbudget verweigert unautorisierte Wiederholung | R02 | halten: Menschliche API-Bindung muss nachgewiesen sein |
| S075 | Worker liefert Artefaktpfad der auf später überschreibbare Datei zeigt | Pfad ist unveränderliches Ergebnis | Contentreferenz und Hash machen nachträgliche Änderung sichtbar | R04 R05 | halten: Hash erlaubt ohne Kopie keine Wiederherstellung |
| S076 | Taskergebnis enthält mehrere Gigabytes Text | Resultate sind klein genug für Eventframes | Begrenztes Ergebnis und sichere Referenz schützen Core | R06 R07 R12 | degradiert: Inhaltsretention braucht separates Budget |
| S077 | Permanent Agent bearbeitet neue Aufgabe während alte auf Permission wartet | Blocked ist funktional idle | Nonterminal-Zuordnung hält Session exklusiv beschäftigt | R01 R10 | halten: Durchsatz sinkt bis zur Entscheidung |
| S078 | Process-Executor Exitcode null obwohl Output leer ist | Prozessende bedeutet fachlichen Erfolg | Exitstatus und Akzeptanzurteil bleiben getrennt | R05 R14 | halten: Domäne muss sinnvollen Gate definieren |
| S079 | Taskprompts enthalten Shell-Metazeichen für Process-Executor | Text kann sicher als Shellkommando interpretiert werden | Explizites argv und validierte Ausführungsspezifikation bleiben prüfbar | R05 R09 R15 | grenze: Shellbedarf erfordert gesondert genehmigte Interpretation |
| S080 | Worker stirbt nach fertigem Artefakt vor Result-Commit | Kein Taskresultat bedeutet keine Arbeit entstanden | Artefakt und Versuch können manuell korreliert werden | R02 R05 R14 | halten: Ergebnisübernahme braucht Evidenz statt automatischer Neuausführung |

## P09 — Externe Geschäftswirkung

Was ist unwiederholbar und was nicht kompensierbar

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S081 | Zahlungsprovider ignoriert Idempotency-Key | Header allein erzwingt Exactly-once | Lokale Intention bleibt und automatische Wiederholung wird verweigert | R02 R09 | halten: Keine garantierte Einmalwirkung ohne Teilnehmervertrag |
| S082 | Email wurde gesendet aber Versandplugin crasht | Kompensation kann Versand rückgängig machen | Versand bleibt als irreversible mögliche Wirkung dokumentiert | R02 R09 | halten: Eine Entschuldigung ist kein Unsend |
| S083 | Mensch genehmigt Rechnung A während Payload auf Rechnung B wechselt | Freigabe gilt dem Tasknamen statt exakten Inhalt | Gebundene Version und Empfänger verhindern Scopewechsel | R05 R09 | halten: Geänderte Rechnung braucht neue Freigabe |
| S084 | Freigabe wird nach Annahme beim Teilnehmer widerrufen | Widerruf kann jede Wirkung noch stoppen | Annahmezeitpunkt und Widerruf bleiben getrennte Fakten | R02 R09 | degradiert: Nur spezifische Kompensation kann Folgen mindern |
| S085 | Saga kompensiert Änderung die inzwischen ein Mensch weiterbearbeitet hat | Inverse Operation darf mutable Gegenwart überschreiben | Versionprecondition verweigert falsche Kompensation | R02 R09 | halten: Manuelle Konfliktlösung statt blindem Rollback |
| S086 | Kompensation schlägt wiederholt fehl | Ein fehlgeschlagener Workflow ist durch Retry heilbar | Durable forward- und compensation-Evidenz bleibt | R02 R10 | halten: Geschäftsschaden bleibt bis menschlicher Intervention |
| S087 | Zwei verschiedene Tasks bestellen fachlich dasselbe Produkt | Task-ID ist globale Geschäftsduplikat-ID | Explizite Businessoperation kann vor Dispatch geprüft werden | R02 R09 R14 | halten: Fachliche Gleichheit braucht Domänenvertrag |
| S088 | Provider berechnet beim Retry einen neuen Preis | Gleicher Payload garantiert gleiche Kosten | Preisversion und Freigabebindung stoppen veränderte Aktion | R07 R09 R13 | halten: Neue Kosten erfordern erneute Entscheidung |
| S089 | Geplanter Reportversand enthält nach Refresh neue vertrauliche Zeilen | Freigabe auf Workflow deckt jeden späteren Inhalt | Gepinnter Versandinhalt und Klassifikation begrenzen Freigabe | R05 R09 R12 | halten: Fachliche Geheimhaltung ist nicht rein syntaktisch prüfbar |
| S090 | Teilnehmer löscht Operationhistorie bevor Factory rekonziliert | Reconciliation bleibt unbegrenzt verfügbar | Lokale Evidenz markiert dauerhaft unbekannten Ausgang | R02 R10 R13 | offen: Kein technischer Beweis des Ausgangs nach Retentionverlust |

## P10 — Operator Aufmerksamkeit und Irrtum

Was funktioniert ohne allwissenden jederzeit verfügbaren Menschen

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S091 | Einziger Operator ist drei Tage offline | Manuelle Recovery ist jederzeit verfügbar | Priorisierte sichere Warteschlange bleibt | R07 R10 | halten: Nicht genehmigte externe Arbeit steht still |
| S092 | Operator bestätigt hundert Dialoge im Alarmsturm reflexhaft | Menschliche Bestätigung ist automatisch informierte Zustimmung | Gebündelte Entscheidungspakete und Pausenregeln bleiben | R09 R10 | halten: Fehlurteile sind nicht vollständig technisch verhinderbar |
| S093 | Vertreter übernimmt ohne Wissen über laufende Außenwirkungen | Zugriffsrecht ersetzt Situationswissen | Korrelierter Handover mit offenem Outcome bleibt lesbar | R02 R05 R10 | halten: Vertretungsautorität muss vorher ausdrücklich erteilt sein |
| S094 | Operator verwechselt Produktions- und Fixture-Instanz | Gleiche CLI-Form schützt vor falschem Ziel | Instanzbindung und eindeutige Effektvorschau begrenzen Verwechslung | R03 R09 | halten: Gewollt falsche Bestätigung bleibt möglich |
| S095 | Operator kopiert Backup per Hand statt Restore-Prozess | Öffnen erkennt automatisch historische DB | Read-only Diagnose zeigt Evidenzgrenze und bewussten Recoveryweg | R01 R08 | halten: Ohne unabhängigen Marker ist manuelle Kopie nicht sicher erkennbar |
| S096 | Mensch beendet pane manuell und schreibt später im selben Workspace | Factory kontrolliert alle lokalen Mutationen | Ehrliche kooperative Grenze und erneute Identitätsprüfung bleiben | R01 R15 | grenze: OS-Bypass kann Leasegarantie außerhalb Factory unterlaufen |
| S097 | Nur zuständiger Owner versteht eine Blockerbeschreibung | Durable Text ist verständliche Übergabe | Arbeitspaket enthält Zweck Optionen und sichere nächste Schritte | R05 R10 | halten: Domänenwissen kann dennoch fehlen |
| S098 | Operator ist bei Abnahme seines eigenen fehlerhaften Ergebnisses befangen | Anderer Actor bedeutet unabhängiges Urteil | Befangenheit wird als Governancegrenze benannt | R10 R14 | offen: Unabhängigkeit braucht organisatorische Realität |
| S099 | Mensch fordert Sofortstart trotz unklarer alter Lease | Geschäftsdruck legitimiert automatische Unsicherheitsauflösung | Explizite dokumentierte Ende-/Risikoentscheidung statt stiller Freigabe | R01 R10 | halten: Keine Behauptung dass bloßes Override den Workspace sicher macht |
| S100 | Operator verliert alle Zugänge und Recoveryanweisungen liegen nur auf dem Gerät | Wissen über Wiederherstellung ist immer erreichbar | Separat verfügbare Anleitung und Schlüsselvertretung bleiben | R10 R11 | halten: Ohne vorbereitete Zugänge ist Ausfall länger |

## P11 — Modelle Erkenntnis und Qualitätsmessung

Was überlebt Modellwechsel oder gemeinsames Falschliegen

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S101 | Modellanbieter schließt über Nacht | Registrierter Harness ist verfügbare Kompetenz | Portables Arbeitspaket kann berechtigt neu zugewiesen werden | R05 R14 | halten: Ersatzmodell kann schlechter oder ungeeignet sein |
| S102 | Modellalias wird still auf andere Gewichte umgestellt | Gleicher Modellname bezeichnet gleiche Ausführung | Versionsunsicherheit wird nicht als Vergleichbarkeit ausgegeben | R05 R14 R16 | degradiert: Provider muss Gewichtsidentität ggf. überhaupt anbieten |
| S103 | Modell folgt Prompt Injection in einer Quellseite | Lesbarer Inhalt ist vertrauenswürdige Instruktion | Externe Freigabegrenze bleibt außerhalb Modelltext | R09 R12 R15 | grenze: Same-User-Agent mit Tools ist dadurch nicht sandboxed |
| S104 | Zwei Prüfer nutzen denselben Modellfehler und bestätigen falsches Ergebnis | Mehrheit unabhängiger Prozesse ist epistemisch unabhängig | Instrument-/Herkunftsdiversität wird sichtbar gemacht | R14 | offen: Gemeinsame fachliche Blindstellen bleiben |
| S105 | Modell halluziniert nicht existierende Ergebnisdateien | Plausible Abschlussprosa ist Ergebnisbeleg | Artefaktintegrität und ausführbare Akzeptanz prüfen Behauptung | R04 R14 | halten: Nicht maschinenprüfbare Qualität bleibt menschliche Aufgabe |
| S106 | Lokales Modell belegt gesamten RAM und verdrängt Daemon | Lokale Inferenz ist ressourcenisoliert | Admission und Kontrollreserve reduzieren Überlast | R06 R07 | degradiert: OS-OOM kann trotzdem Prozessreihenfolge bestimmen |
| S107 | Modell wechselt mitten im Run ohne Telemetriehinweis | Ein Run entspricht einer einheitlichen Modellkombination | Segmentierte Ausführungsevidenz erhält ehrliche Attribution | R05 R14 | degradiert: Nicht beobachtbare Wechsel machen Run nicht vergleichbar |
| S108 | Modell produziert unendlich viele korrekte Zwischenschritte | Fortschritt pro Turn garantiert Terminierung | Lauf-/Kostenbudget führt zu sicherem Halt | R07 R10 | halten: Kein automatisches Urteil über nützlichen Teilfortschritt |
| S109 | Kontextfenster schrumpft nach Harnessupgrade | Byte-stabiler Kontext passt in jedes Modell | Vorabkapazität und Quelleninventar verhindern stillen Sicherheitsverlust | R05 R06 R16 | halten: Kürzung braucht neue explizite Kontextentscheidung |
| S110 | Benchmarkfixture wird vom Modell erkannt und speziell ausgetrickst | Guter Benchmarkscore beweist allgemeine Qualität | Getrennte Gates und adversarielle Vergleichsfälle begrenzen Aussage | R14 | offen: Keine allgemeine Kompetenzgarantie aus endlichem Korpus |

## P12 — Kontext Wissen und Provenienz

Welche Inputs und Instruktionen gelten für welchen Lauf

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S111 | Root-AGENTS wird zwischen Queueing und Start geändert | Deterministische Compilation ist historische Reproduzierbarkeit | Geplante und tatsächlich verwendete Revision bleiben unterscheidbar | R05 | halten: Heutige Sicherheitsregeln dürfen nicht umgangen werden |
| S112 | Ein AGENTS-Quellfile ist plötzlich unlesbar | Fehlender Kontext kann als leer ersetzt werden | Compilation stoppt mit benannter fehlender Quelle | R05 R08 | halten: Keine Zustellung mit still reduzierter Instruktion |
| S113 | Human schreibt Geheimnis in Taskprompt | Operatorinput ist automatisch persistenzsicher | Datenminimierung und Incidentweg begrenzen Weiterverbreitung | R12 | verlust: Präventionsprüfung kann Secrets nicht sicher vollständig erkennen |
| S114 | Knowledge-Link führt zyklisch durch tausende Notizen | Mehr Retrieval liefert monoton besseren Kontext | Explizite begrenzte Quellen bleiben reproduzierbar | R05 R06 | halten: v1 folgt ohnehin keinen solchen Knowledge-Links |
| S115 | Zwei Quellen widersprechen sich über aktuelle Freigaberegel | Konkatenation löst Policykonflikte | Quellenrevisionen und aktuelle Authorisierung bleiben getrennt | R05 R09 | halten: Textkonflikt braucht menschliche Klärung |
| S116 | Quellwebsite ändert Inhalt ohne URLänderung | URL ist immutable Referenz | Erfasste sichere Contentrevision bleibt überprüfbar | R04 R05 | halten: Nicht sicher aufbewahrbarer Inhalt begrenzt Replay |
| S117 | Repo enthält bösartige native Harnesskonfiguration | Factory-Kontext ist der gesamte tatsächlich geladene Kontext | Effektfreigabe und ehrliche Trustgrenze bleiben | R05 R15 | grenze: Fremdrepositories brauchen Isolation vor Ausführung |
| S118 | Agent schreibt Shared Memory im Namen fremden Scopes | Dateipfad bestimmt berechtigten Autor | Actor- und Scopebindung begrenzen legitime API-Schreibpfade | R09 R12 | grenze: Direkter Same-User-Dateizugriff bleibt technisch möglich |
| S119 | Artefaktnamen unterscheiden sich nur durch Unicode-Normalisierung | Textgleichheit ist überall dieselbe Identität | Manifest validiert Zielkollisionen vor Übernahme | R04 R16 | halten: Automatische Normalisierung kann bestehende Links ändern |
| S120 | Context wird zum Debuggen vollständig in Telemetrie kopiert | Observabilitydaten sind weniger sensibel als Produktdaten | Getrennte Payload-/Trace-/Sample-Retention begrenzt Kopien | R07 R12 | verlust: Bereits kopierte sensible Daten brauchen Incidentbehandlung |

## P13 — Angreifer und Supply-Chain-Missbrauch

Wo endet kooperative Policy und beginnt notwendige Isolation

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S121 | Plugin läuft als gleicher User und liest Keychain außerhalb Kernel | Capabilities sind eine Betriebssystemsandbox | Nur ehrliche Ablehnung feindlicher Plugins bleibt versprechbar | R15 | grenze: Keine Geheimnisgarantie nach Same-User-Kompromittierung |
| S122 | Angreifer ersetzt Pluginbinary nach Manifestprüfung | Geprüfter Pfad bedeutet geprüftes Executable beim Start | Herkunftsbindung und Aktivierungsprüfung begrenzen Supply-Chain-Fenster | R06 R15 R16 | grenze: OS-Angreifer kann lokale Prüfer ebenfalls manipulieren |
| S123 | Transportplugin behauptet falschen authentifizierten Actor | Jeder Plugin-Rückruf ist vertrauenswürdige Authentisierung | Gebundene Aktivierungsfähigkeiten begrenzen Reverse-RPC | R06 R09 R15 | grenze: Kompromittierter autorisierter Transport bleibt mächtig |
| S124 | Agent schreibt absichtlich direkt in SQLite | Nur API-Mutationen existieren technisch | Ehrlicher kooperativer Produktvertrag statt Scheinschutz | R15 | grenze: Hard Isolation ist separates Deploymentprojekt |
| S125 | SQL-/Pfadinjection steckt in neuem Pluginparameter | Validiertes Envelope validiert automatisch Semantik | Typisierte Payloads und begrenzte Fähigkeiten reduzieren Fläche | R06 R09 | grenze: Operation-spezifische Validierung bleibt notwendig |
| S126 | Log enthält Terminalescapes die Diagnoseanzeige fälschen | Textdiagnose ist passive Darstellung | Sichere begrenzte Darstellung trennt Daten von Bedienung | R06 R08 R12 | degradiert: Renderer muss Escape- und Linkverhalten konkret prüfen |
| S127 | Altes gestohlenes Capability-Token wird nach Restore erneut gültig | Snapshot-Auth ist gegenwärtige Auth | Neue Inkarnation und aktuelle Berechtigung sperren alte Bindung | R01 R03 R09 | halten: Externe Teilnehmer müssen Widerruf oder Ablauf respektieren |
| S128 | Bekannte Integration wird per Dependency-Typosquatting ersetzt | Paketname belegt Herkunft | Installationsprovenienz und explicit enable begrenzen Annahmen | R06 R16 | halten: Signatur allein beweist keinen gutartigen Inhalt |
| S129 | Ransomware verschlüsselt DB Backups und Knowledge gleichzeitig | Lokale Backups sind vor gleichem User geschützt | Unabhängig geschütztes Recovery-Set bleibt | R11 R15 | verlust: Keine Sicherheit für bereits exfiltrierte Daten |
| S130 | Task erzeugt riesige gültige Delegationsketten ohne Zyklus | Azyklisch heißt ressourcenbeschränkt | Admission und Eingabegrenzen halten Core innerhalb Budget | R06 R07 R09 | degradiert: Legitime sehr große Aufträge brauchen bewusste Aufteilung |

## P14 — Datenschutz Recht Beweispflichten

Welche widersprüchlichen Pflichten kann Architektur nicht selbst entscheiden

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S131 | Betroffene Person verlangt Löschung aus append-only History | Unveränderlichkeit und vollständige Löschung sind gleichzeitig trivial | Nicht sensible Metadaten und benannte Evidenzlücke bleiben | R04 R12 | offen: Juristische Abwägung und Replayverlust sind explizit zu entscheiden |
| S132 | Gericht verlangt Legal Hold während Retention löschbereit ist | Ein allgemeines TTL löst jede Aufbewahrungspflicht | Klassifizierte Halte-/Löschentscheidung bleibt auditierbar | R10 R12 | offen: Kein automatischer Rechtsentscheid |
| S133 | Kunde verlangt Datenresidenz die aktueller Modellprovider nicht erfüllt | Ein austauschbarer Provider ist überall zulässig | Gepinnte Ausführung wird vor Dispatch gegen aktuelle Policy geprüft | R05 R09 R12 | halten: Providervertrag und Rechtslage brauchen menschliche Prüfung |
| S134 | Backup enthält Geheimnisse die im Original schon rotiert wurden | Rotation beseitigt alle gespeicherten Geheimnisse | Separater Incident- und Schlüsselprozess begrenzt weitere Nutzung | R11 R12 | verlust: Kopien können außerhalb eigener Kontrolle fortbestehen |
| S135 | Lizenz einer notwendigen Library wird für neue Releases unbrauchbar | Technische Portabilität ist rechtliche Weiterverwendbarkeit | Gepinnte Herkunft und Export halten Ausstieg möglich | R05 R16 | halten: Rechtliche Zulässigkeit ist nicht durch Hash bewiesen |
| S136 | Audit muss beweisen welcher Mensch konkret freigegeben hat | Ein hochgezählter authorisations-Counter genügt als Audit | Actor-gebundene immutable Freigabeevidenz bleibt | R02 R04 R09 | halten: Authentisierung muss die reale Person hinreichend binden |
| S137 | Behörde beschlagnahmt Originalgerät und fordert Einsicht | Physischer Besitz und Geheimhaltung bleiben unter eigener Kontrolle | Unabhängige Recovery erlaubt Kontinuität im rechtlich zulässigen Rahmen | R11 R12 | offen: Keine technische Umgehung rechtmäßiger Zugriffsanforderungen geplant |
| S138 | Ein ursprünglich harmloses Feld wird später als sensibel eingestuft | Datenklassifikation ist zeitlich unveränderlich | Referenzinventar erlaubt gezielte Neubewertung | R04 R12 | offen: Historische Kopien und Replay können betroffen sein |
| S139 | Auskunftsanfrage verlangt Provenienz über gelöschte Scopes hinweg | Scope-Retirement darf Zugehörigkeit vergessen | Erhaltene nicht sensible Historie erlaubt begrenzte Auskunft | R04 R12 R16 | halten: Vorher gelöschte zulässige Inhalte bleiben nicht rekonstruierbar |
| S140 | Versicherer verlangt beweisbare Recovery statt vorhandener Backups | Dokumentierter Plan ist durchgeführter Drill | Messbare Wiederherstellung liefert konkrete Evidenz | R04 R11 R16 | halten: Diese Analyse selbst ist kein solcher Nachweis |

## P15 — Ökonomie Kapazität und SLA

Welche Automation ist unter realen Budgets noch tragfähig

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S141 | Tokenpreise steigen über Nacht um Faktor hundert | Gleiche Arbeit hat vorhersehbare Stückkosten | Kostenbudget stoppt neue Aufnahme oder Dispatch | R07 R09 | halten: Bisher entstandene Kosten bleiben |
| S142 | Provider ersetzt Prepaid durch unbekannte nachträgliche Abrechnung | Lokaler Tokenzähler ist vollständige Kostenkontrolle | Unbekannte Preise führen zu expliziter Freigabegrenze | R07 R09 R13 | halten: Keine harte Kostenobergrenze ohne Teilnehmerunterstützung |
| S143 | Ein großer Scope monopolisiert alle Slots mit legalen Tasks | Per-Agent-Limit garantiert globale Fairness | Globale Admission schützt andere Scopes | R07 | degradiert: Fairnesspolicy muss Zielkonflikte offen festlegen |
| S144 | Jeder kleine Task kostet mehr menschliche Freigabe als er spart | Sichere Automation ist automatisch wirtschaftlich | Messbare Interventionslast erlaubt bewussten Betriebsstopp | R10 R14 | offen: Freigaben dürfen nicht wegen Rentabilität automatisch entfallen |
| S145 | Nachfrage verzehnfacht sich ohne weitere Workspaces | Mehr max_sessions erzeugt mehr sichere Parallelität | Queue bleibt begrenzt statt Workspace-Multiplexing | R01 R07 | degradiert: Worktrees werden nicht automatisch angelegt |
| S146 | Budget wird während langer Agentarbeit auf null gesetzt | Bereits laufende Arbeit kann risikolos hart beendet werden | Kooperativer Halt und offene Effektabsicht bleiben | R02 R07 R09 | halten: Bereits angenommene Kosten oder Effekte lassen sich nicht annullieren |
| S147 | Geschäftskunde verlangt garantierte Echtzeitantwort bei lokalem Single-Host | Best-effort Architektur kann per SLA Echtzeit werden | Ehrliche Einsatz- und Verfügbarkeitsgrenze bleibt | R15 R16 | grenze: Neues SLA braucht anderes Mandat und reale Kapazitätsbelege |
| S148 | Ein Fachauftrag ist nach Queuewartezeit wirtschaftlich wertlos | Persistente Arbeit sollte irgendwann immer ausgeführt werden | Explizite Gültigkeit erlaubt sichtbaren Verfall | R05 R13 | degradiert: Abschreiben ist Geschäftsentscheidung |
| S149 | Unternehmen kann keinen externen Backupdienst mehr bezahlen | Durability hat keine laufenden Betriebskosten | Export und geplante Stilllegung erhalten vorhandene Evidenz | R11 R16 | halten: Kein Fortbestandsversprechen ohne finanzierte Ausfalldomäne |
| S150 | Unbekannte Providerrechnung macht Metrikvergleich irreführend | Gemessene Tokens sind vollständige Wirtschaftlichkeit | Nicht vergleichbare Läufe werden als solche markiert | R05 R07 R14 | degradiert: Fehlende Kosten werden nicht als null verbucht |

## P16 — Entwicklung Release und Wartbarkeit

Welche Verträge überleben Versions- und Maintainerwechsel

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S151 | Rust-Update ändert Plattformverhalten trotz unverändertem Code | Lockfile allein fixiert alle Laufzeiteigenschaften | Buildherkunft und Konformitätsfixtures halten Unterschiede sichtbar | R06 R16 | halten: OS und Hardware bleiben zusätzliche Variablen |
| S152 | Release enthält neue Eventversion aber keinen alten Upcaster | Forward Migration garantiert historische Lesbarkeit | Kompatibilitätskorpus verweigert unvollständiges Release | R04 R16 | halten: Nicht gespeicherte alte Semantik ist nicht nachträglich herleitbar |
| S153 | Python-Prototyp und Rust mutieren denselben Tabellennamen mit anderem Schema | SQLite-Serialisierung vereinheitlicht Domänenmodelle | Eindeutiger Cutover verweigert konkurrierenden Writer | R03 R16 | halten: Legacymigration ist separate genehmigte Operation |
| S154 | Rollback des Binaries trifft auf bereits migriertes Schema | Rollback bedeutet alte Executable starten | Read-only Kompatibilitätsfehler bleibt statt stiller Rückmigration | R04 R08 R16 | halten: Rollbackplan muss Datenzustand einbeziehen |
| S155 | Ein Versionspaket ist nach Maintainerwechsel nicht mehr herunterladbar | Referenzierte Software bleibt dauerhaft verfügbar | Erlaubt archivierte Builds und lesbarer Export erhalten Recovery | R11 R16 | halten: Archivierung kann Lizenzgrenzen haben |
| S156 | Veröffentlichung exportiert versehentlich Company-root statt Projekt | Gitremote-Konvention verhindert jede Fehlveröffentlichung | Releasegrenze und Herkunftsprüfung begrenzen freigegebenen Inhalt | R12 R16 | verlust: Bereits veröffentlichte Geheimnisse bleiben Incident |
| S157 | Tests bestätigen absichtlich unsicheren Missing-ID-Fallback | Grüne Tests bestätigen richtige Invariante | Adversarielle Vertragsprüfung kann falschen Oracle offenlegen | R01 R14 R16 | offen: Testzahl allein misst keine Sicherheitsqualität |
| S158 | README und ADR widersprechen dem aktuellen Library-Schema | Mehr Dokumentation bedeutet mehr Klarheit | Versionierter Vertragsstatus benennt Architektur versus Implementierung | R04 R16 | halten: Erfordert laufende Pflege statt weiterer Schattenbacklogs |
| S159 | Automatischer Updater aktiviert Plugin vor Manifestverifikation | Installation und Aktivierung sind derselbe sichere Schritt | Explicit enable und Kompatibilitätsprüfung trennen beide | R06 R16 | halten: Kein Upgrade ohne passende Liefergrenze |
| S160 | Jedes Architekturproblem wird durch weiteren Kerndienst beantwortet | Mehr Mechanismen erhöhen immer Resilienz | Residues werden in bestehende Zuständigkeiten komponiert | R07 R16 | offen: Komplexitätsbudget und bewusste Nicht-Unterstützung bleiben Entscheidungen |

## P17 — Organisation Verantwortung und Macht

Was geschieht wenn nicht Hardware sondern Zuständigkeit wechselt

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S161 | Firmengründer fällt dauerhaft aus und niemand besitzt Freigabeautorität | Menschlicher Owner ist eine dauerhafte Infrastrukturkomponente | Expliziter Nachfolgeprozess und lesbare offene Entscheidungen bleiben | R10 R11 | halten: Ohne legitimierte Nachfolge bleibt externe Tätigkeit gestoppt |
| S162 | Zwei Geschäftsführer geben widersprüchliche Anweisungen zum selben Effekt | Menschliche Intention ist stets eindeutig | Konflikt wird vor neuer Wirkung sichtbar blockiert | R09 R10 | halten: Kernel darf keine gesellschaftsrechtliche Rangfolge erfinden |
| S163 | Unternehmen wird verkauft und bisherige Mitarbeiter verlieren Zugriffsrechte | Historische Zugehörigkeit gewährt künftige Rechte | Alte Provenienz bleibt und aktuelle Policy sperrt Dispatch | R05 R09 R12 | halten: Übergang braucht freigegebene Zuständigkeiten und Retention |
| S164 | Projekt wird ausgegliedert und darf Company-Kontext nicht mitnehmen | Portables Arbeitspaket darf überall kopiert werden | Klassifizierter Export hält zulässige Intention statt kompletten Kontext | R05 R12 R16 | halten: Portabilität ist der Vertraulichkeit untergeordnet |
| S165 | Sicherheitsverantwortlicher und Produktowner streiten über riskanten Resume | Ein technisches Override ersetzt Risikoentscheidung | Begrenzte Befugnis bleibt bis benannter Konfliktentscheidung | R02 R09 R10 | halten: Kein automatisches Mehrheitsvotum für irreversiblen Effekt |
| S166 | Dienstleister ersetzt komplette Agentflotte mit anderen Instrumenten | Gleiche Rollennamen bedeuten gleiche Qualität und Beobachtung | Pinned Arbeit und Instrumentversionen bleiben vergleichbar begrenzt | R05 R14 R16 | degradiert: Neue Instrumente brauchen eigene Kalibrierung |
| S167 | Teams optimieren auf möglichst wenige blocked Runs und umgehen Freigaben | Eine niedrige Blockerzahl ist immer besser | Sicherheitsverletzung und Interventionslast werden getrennt bewertet | R09 R10 R14 | offen: Anreizsystem ist nicht nur ein Kernelproblem |
| S168 | Scope-Besitzer verweigert Budgetauskunft obwohl gemeinsame Maschine überlastet ist | Autonome Scopes regulieren gemeinsamen Verbrauch selbst | Globale Admission setzt gemeinsame Grenzen | R07 R10 | degradiert: Fairness und Prioritätskonflikte brauchen Ownerentscheidung |
| S169 | Organisation verlangt nachträgliches Umschreiben eines unbequemen Auditfakts | Korrektur bedeutet alte Wahrheit ersetzen | Append-only Korrektur hält ursprüngliche Provenienz sichtbar | R04 R10 R12 | halten: Legale Löschung muss gesondert behandelt werden |
| S170 | Ein Partner stellt Betrieb ein und niemand kann seine alten Operationen bestätigen | Businesspartner existieren so lange wie lokale Tasks | Dauerhaft unbekannter Outcome bleibt statt neuem Retry | R02 R10 | offen: Automatische Rekonziliation kann endgültig unmöglich werden |

## P18 — Wachstum Plattformwechsel und neue Nutzung

Welche zukünftige Anforderung sprengt die heutige Einsatzgrenze

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S171 | Tausend neue Scopes erzeugen je einen Pluginprozess | Scope-lokale Isolation skaliert ohne globale Kosten | Admission begrenzt Aktivierungen und hält Controlkapazität | R06 R07 | degradiert: Multiplexing wäre neuer expliziter Vertrag statt heimlicher Optimierung |
| S172 | Zwei Kunden fordern gegenseitige Geheimhaltung auf derselben User-ID | Scope-Rechte sind harte Mandantentrennung | Heutige Trusted-User-Nutzung bleibt eng begrenzt | R15 | grenze: Mandantenbetrieb benötigt eigenes Isolationsmandat |
| S173 | Remote-Worker laufen über Tage offline auf fremder Hardware | Lokale Lease beschreibt entfernte Realität vollständig | Operationen bleiben gesperrt bis sichere Inkarnations-/Effektprüfung | R01 R03 R13 | grenze: Remote Runtime ist nicht automatisch durch lokales Modell abgedeckt |
| S174 | Produkt muss auf Smartphone ohne Daemon-Dauerbetrieb laufen | Residenter Mutator ist auf jeder Plattform verfügbar | Portabler Arbeits-/Exportvertrag überlebt ohne dortige Automatisierung | R05 R15 R16 | grenze: Plattformwechsel benötigt explizite Architektur statt verdeckter zweiter Writer |
| S175 | Firma verlangt Active-active mit automatischem Failover zwischen Städten | Lokaler Lock kann verteiltes Konsensproblem lösen | Single-active bleibt ehrliche Betriebsgrenze | R03 R15 R16 | grenze: Kein Konsenscluster aus dieser Analyse genehmigt |
| S176 | Robotikplugin steuert irreversible Bewegung mit Millisekundenfrist | Approval-gated Büroautomation ist Echtzeit-Sicherheitssteuerung | Factory kann nur nichtkritische Planung übergeben | R09 R15 | grenze: Safety-zertifizierter Controller liegt außerhalb dieses Produkts |
| S177 | Regulierter Betrieb verlangt manipulationssichere Beweise gegen Root-Admin | Lokales append-only SQLite ist unabhängiger Trustanchor | Unveränderlichkeitsvertrag bleibt nur im kooperativen Trustmodell | R15 | grenze: Externer Trustanchor und andere Deploymentgrenze nötig |
| S178 | Ereignisbestand wächst auf Jahrzehnte und Replay dauert länger als RTO | Rebuildbar bedeutet schnell genug wiederherstellbar | Validierte Snapshots plus vollständige Historie können Recovery beschleunigen | R04 R07 R11 | degradiert: Beschleuniger brauchen Integritätsbeleg und echtes Zeitbudget |
| S179 | Kunde fordert vollständigen Export und Abschaltung aller Factorydienste | Produktbindung ist Voraussetzung für Lesbarkeit eigener Arbeit | Offene Artefakt-/Eventformate und Stilllegungsplan erhalten Evidenz | R05 R11 R16 | degradiert: Native Harnessgespräche sind nicht vollständiger Factoryexport |
| S180 | Pluginlandschaft entwickelt wechselseitige Abhängigkeiten mit Startzyklus | Einzelne valide Aktivierungen ergeben einen startbaren Graph | Bootstrap und Quarantäne halten Minimalbetrieb ohne Zyklus | R06 R08 | degradiert: Optionalitätsgraph muss vor Aktivierung geprüft werden |

## P19 — Abwegige Extrem- und Langzeitbedingungen

Welche verborgenen Selbstverständlichkeiten sind keine Naturgesetze

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S181 | Extremer Sonnensturm zerstört Elektronik in mehreren Backupregionen | Geografische Trennung beseitigt jeden Common-Cause-Ausfall | Nur tatsächlich überlebende Offlinekopien bleiben Informationsquelle | R11 | verlust: Keine Garantie gegen Verlust aller physischen Kopien |
| S182 | Mac erwacht im Jahr 2046 und alle Termine Zertifikate und Anbieter sind historisch | Lange Pause ist nur ein großer gewöhnlicher Restart | Read-only Rekonstruktion und Export bleiben ohne automatischen Catch-up | R01 R11 R13 R16 | halten: Aktuelle Berechtigung und ausführbare Software müssen neu begründet werden |
| S183 | Nach zwanzig Jahren versteht niemand mehr Sprache und Geschäftskürzel der Taskprompts | Gespeicherte Bytes bleiben verständliche Bedeutung | Versioniertes Glossar und erklärbare Arbeitspakete erleichtern begrenzte Rekonstruktion | R05 R11 R16 | offen: Semantik kann trotz intakter Bytes unwiederbringlich fehlen |
| S184 | Alle verfügbaren Modelle und menschlichen Prüfer teilen denselben überzeugenden Irrtum | Übereinstimmung beweist Wahrheit | Nur provenancegetrennte Aussagen bleiben ehrlich archiviert | R14 | offen: Keine universelle Wahrheitsmaschine aus der Inzidenzmatrix |
| S185 | Zwei technisch identische Instanzklone erhalten widersprüchliche legitime Weltzustände | Deterministische Software kann widersprüchliche externe Wahrheiten vereinigen | Single-active-Grenze und Konfliktevidenz bleiben statt automatischer Zusammenführung | R03 R09 R10 | halten: Fachliche Konfliktentscheidung ist nicht aus lokalen IDs ableitbar |
| S186 | Neue Rechtslage verlangt sofortige Löschung aller Geschäftsdaten und zugleich ewige Beweisbarkeit | Jede Anforderungskombination ist architektonisch erfüllbar | Widerspruch wird als nicht erfüllbares Mandat offengelegt | R10 R12 | offen: Keine Architektur kann beide absoluten Forderungen erfüllen |
| S187 | Energie steht künftig nur fünf Minuten pro Monat zur Verfügung | Dauerdaemon und kontinuierliche Beobachtung sind natürliche Gegebenheiten | Offline lesbares Recovery-Set und gezielter Export bleiben | R11 R13 R15 | grenze: Betriebsmodell müsste explizit neu entworfen werden |
| S188 | Alle genutzten Kryptoprimitiven gelten plötzlich als praktisch gebrochen | Integritäts- und Authentizitätsbelege sind für immer vertrauenswürdig | Metadaten über verwendete Algorithmen erlauben geordnete Neubewertung | R09 R11 R16 | verlust: Alte Vertraulichkeit und Signaturbeweise können nicht rückwirkend repariert werden |
| S189 | Firma spaltet sich rechtlich in zwei Nachfolger die beide alle Freigaberechte beanspruchen | Ein stabiler Owner ist aus dem Instanznamen ableitbar | Historische Provenienz und gesperrte neue Wirkungen bleiben | R09 R10 R12 | offen: Eigentums- und Zugriffsentscheidung benötigt legitime externe Autorität |
| S190 | Alle Geräte Backups Schlüssel und Erinnerungen an das System verschwinden | Ein gutes Recoveryprotokoll kann Information aus nichts erzeugen | Es bleibt keine rekonstruierbare Factory-Arbeit | R11 R15 | verlust: Bewusst akzeptierte informationstheoretische Grenze statt falschem Recoveryversprechen |

## P20 — Kaskaden und widersprüchliche Signale

Welche Reihenfolge mehrerer Stressoren besiegt die Einzelmaßnahmen

| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |
|---|---|---|---|---|---|
| S191 | Alter Restore trifft auf widerrufene Freigabe und inzwischen ausgeführte queued-Aktion | Leerer Snapshotversuch plus alte Freigabe beweist sichere Ausführung | Recovery-Epoche sperrt Wirkung bis aktuelle Evidenz vorliegt | R01 R02 R09 | halten: Ohne unabhängige Teilnehmerhistorie ist Ausgang dauerhaft unentscheidbar |
| S192 | Pluginflood füllt SSD genau als dringender Widerruf eingeht | Sicherheitskommandos haben unter Überlast automatisch Vorrang | Vorab reservierte Kontrollkapazität erlaubt begrenzten Stoppversuch | R02 R06 R07 R09 | halten: Ohne Commit darf Widerruf nicht als dauerhaft bestätigt werden |
| S193 | Herdrausfall hält Leases und einziger Operator stirbt während Rechenbudget ausläuft | Sicherer Halt besitzt stets einen erreichbaren Fortsetzungsweg | Taskevidenz und legitimierter Nachfolgeprozess bleiben | R01 R07 R10 R11 | halten: Ohne Nachfolge keine automatische Freigabe oder Fortsetzung |
| S194 | Restoremanifest ist vollständig aber einzige Contentkopie unterliegt gerichtlich angeordneter Löschung | Technische Vollständigkeit legitimiert jede Wiederherstellung | Nicht sensible Historie und explizite Replaylücke bleiben | R04 R11 R12 | offen: Rechtliche Entscheidung kann technische Recovery bewusst begrenzen |
| S195 | Providerwechsel verändert Outputformat und derselbe Modelltyp verifiziert seinen Fehler | Portabilität plus zweiter Agent garantiert gleiche Qualität | Gepinnte Instrumente und unabhängige Kriterien halten Zweifel sichtbar | R05 R14 | offen: Kein fachlicher Erfolg ohne tragfähigen unabhängigen Maßstab |
| S196 | Zwei Scopeagenten vervielfachen Tasks während Cron eine monatelange Nachholwelle erzeugt | Einzelne Loopprüfung oder Scheduler-Deduplikation begrenzt Gesamtlast | Globale Admission und faire kontrollierte Catch-up-Queue bleiben | R07 R09 R13 | degradiert: Fachlich doppelte verschiedene Tasks können weiter manuelles Review brauchen |
| S197 | Compensation läuft gegen vom Menschen veränderte Daten während ihr Plugin geupdatet wird | Inverse Operation bleibt trotz Welt- und Vertragswechsel gleich | Gepinnte Operation und Versionsprecondition verweigern falsche Rückabwicklung | R02 R06 R09 R16 | halten: Geschäftlicher Schaden kann trotz technisch korrektem Halt bleiben |
| S198 | Migrationscutover wird unterbrochen und Backupklon startet parallel den Legacydispatcher | Ein erfolgreicher lokaler Lock beendet alte Mutationsautorität überall | Expliziter Writer-Cutover und Teilnehmerfencing begrenzen Doppelwirkung | R01 R02 R03 R16 | halten: Ohne Fencing muss exklusive Betriebsautorität manuell nachgewiesen werden |
| S199 | Geräteverlust trifft auf verlorenen Backup-Key und insolventen Softwarelieferanten | Unabhängige Datenkopie allein macht Recovery möglich | Nur vorher gesicherte Schlüsselvertretung und lesbare Formate können helfen | R11 R16 | verlust: Fehlen diese bereits ist Wiederherstellung unter Umständen unmöglich |
| S200 | Incidentbericht behauptet volle Resilienz weil alle Matrixfelder markiert sind und stoppt reale Tests | Syntaktische Abdeckung ist empirischer Sicherheitsbeweis | Offene Evidenzklassen und unabhängiger Review halten Modellgrenze sichtbar | R14 R16 | offen: Diese selbstreferenzielle Karte widerlegt keine unbekannten weiteren Blindstellen |

