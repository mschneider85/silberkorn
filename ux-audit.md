# UX-Konsistenz-Audit (17.10.2026)

Pfade beziehen sich auf `crates/sk-app/src/`, sofern nicht anders angegeben. Die Zeile
„Stand 18.10.“ je Punkt gibt den Abgleich mit Commit `e83bcb5` wieder (offen / teilweise / behoben)
und die aktuelle Stelle. Die übrigen Zeilennummern stammen vom Stand 17.10.2026 (inklusive der nicht committeten Änderungen an der
Trapezkorrektur). Mit „bestätigt“ markierte Punkte sind im Code nachgeprüft, die übrigen
stammen aus der Code-Durchsicht und wurden nicht im laufenden Programm ausprobiert.

Entscheidung je Punkt: das zutreffende Kästchen ankreuzen (`[x]`), bei Bedarf eine Notiz
ergänzen.

---

## A. Fehler mit direkter Wirkung

### 1. Trapezregler kürzt alte Werte still auf ±50 (bestätigt)
- **Stand 18.10.:** **behoben** – clamp_existing_to_range(false) – aktuelle Stelle: `widgets.rs:454`
- **Stelle:** `widgets.rs:380` (`slider_row_with`), `develop_tools.rs:376-378`, `KEYSTONE_MAX` in `sk-core/src/params.rs:801`
- **Befund:** egui 0.36 begrenzt bei `DragValue` vorhandene Werte standardmäßig auf den Bereich (`clamp_existing_to_range = true`). Eine ältere Bearbeitung mit ±100 wird schon beim Zeichnen der aufgeklappten Perspektive-Sektion auf ±50 gesetzt, das Bild ändert sich ohne Zutun. Widerspricht dem Doc-Kommentar „still render as before“.
- **Vorschlag:** `.clamp_existing_to_range(false)` in `slider_row_with` oder Bereich auf `max(|wert|, KEYSTONE_MAX)` erweitern.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): `clamp_existing_to_range(false)` in `slider_row_with`

### 2. Esc schließt nicht-modale Fenster nicht (bestätigt)
- **Stand 18.10.:** **behoben** (Paket A) – Kopieren und Tastenkürzel sind jetzt Modals und schließen mit Esc; „Über Silberkorn“ schließt weiter nicht mit Esc – aktuelle Stelle: `app.rs:2423, splash.rs:154`
- **Stelle:** `app.rs:2376` (Esc per `consume_key`), danach `clipboard.rs:169`, `shortcut_editor.rs:201`, `splash.rs:154` (nur `key_pressed`)
- **Befund:** „Anpassungen kopieren“, „Tastenkürzel“ und „Über Silberkorn“ reagieren nicht auf Esc, weil `shortcuts()` die Taste vorher abholt. Esc beendet stattdessen Werkzeuge oder das Vollbild dahinter. „Über“ schließt mit Enter, aber nicht mit Esc. Modals schließen dagegen korrekt per `should_close()`.
- **Vorschlag:** Diese Fenster im Shortcut-Schutz berücksichtigen bzw. Esc vor `shortcuts()` auswerten.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): „Über Silberkorn“ blockiert die Tastenkürzel wie ein Dialog, Esc schließt es

### 3. Tastenkürzel wirken hinter nicht-modalen Fenstern
- **Stand 18.10.:** **behoben** (Paket A) – Kopieren/Tastenkürzel abgeschirmt; hinter dem Über-Fenster wirken Kürzel weiter – aktuelle Stelle: `app.rs:2409`
- **Stelle:** `app.rs:2361`
- **Befund:** Der Schutz greift nur bei Popup, Modal, Palette oder Textfeld. Hinter Kopier-Fenster, Tastenkürzel-Fenster und Über-Splash laufen C, G, Entf, Strg+Umschalt+V usw. Widerspricht dem Kommentar dort.
- **Vorschlag:** Diese Fenster in die Schutzbedingung aufnehmen oder als Modal umbauen (siehe 46).
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): Kürzel wirken auch hinter dem Über-Fenster nicht mehr

### 4. Strg+K im Vollbild sperrt die Tastatur
- **Stand 18.10.:** **behoben** (Paket A) – aktuelle Stelle: `app.rs:2401-2403, Vollbild-Zweig app.rs:2769-2780`
- **Stelle:** `app.rs:2355`, Vollbild-Zweig `app.rs:2700-2711`
- **Befund:** Die Palette wird geöffnet, im Vollbild aber nicht gezeichnet. `shortcuts()` bricht wegen `palette.open` ab, auch Esc. Nur ein zweites Strg+K hilft. Außerdem öffnet Strg+K die Palette auch über einem offenen Modal, deren Befehle laufen dann hinter dem Dialog.
- **Vorschlag:** Palette auch im Vollbild zeichnen; Strg+K bei offenem Modal ignorieren.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): Befehlssuche wird im Vollbild gezeichnet; Strg+K über Dialogen wirkungslos

### 5. Ein Undo-Schritt pro Mausrad-Raste und Pfeiltaste (bestätigt)
- **Stand 18.10.:** **behoben** – Mausrad-/Pfeiltasten-Schritte werden bis 0,8 s Pause zu einem Undo-Schritt gebündelt (db46af1) – aktuelle Stelle: `app.rs:2502-2528, widgets.rs:680-713`
- **Stelle:** `app.rs:2458-2460`; Mausrad `widgets.rs:497-514`, Pfeiltasten `widgets.rs:466-481`, Schnellbearbeitung `browser.rs:139-155`, `:548`
- **Befund:** Festgeschrieben wird, sobald keine Maustaste gedrückt ist. Jede Raste bzw. jeder Tastendruck wird ein eigener Schritt; 20 Rasten verdrängen ältere Schritte über `UNDO_LIMIT`. Ein Mauszug ergibt dagegen genau einen Schritt.
- **Vorschlag:** Mausrad/Tastatur-Änderungen am selben Regler zeitlich bündeln (z. B. Commit erst nach ~500 ms Ruhe oder beim Fokuswechsel).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 6. Änderungen an mehreren Bildern nicht rückgängig zu machen
- **Stand 18.10.:** **offen** – aktuelle Stelle: `library.rs:346-375`
- **Stelle:** `library.rs:345-376` (`set_adjustments_on`); Aufrufer u. a. `clipboard.rs:83`, `:161`
- **Befund:** Einfügen auf Auswahl, Stil anwenden, Auto-Anpassen, Look angleichen, Werkzeug übertragen, Rauschen übertragen und Prozessversion schreiben die Sidecars nicht geöffneter Bilder direkt. Strg+Z nimmt nur das aktuelle Bild zurück. Es gibt auch keine Rückfrage.
- **Vorschlag:** Mehrbild-Änderung als ein Undo-Schritt (alte Sidecars merken) oder zumindest Bestätigung ab n Bildern.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 7. Bewertung, Farb-Label und Stichwörter ohne Undo
- **Stand 18.10.:** **offen** – aktuelle Stelle: `library.rs:216`
- **Stelle:** `library.rs:216` (`update_meta`)
- **Befund:** Läuft an der History vorbei. Strg+Z direkt nach einer Bewertung nimmt stattdessen die letzte Bildbearbeitung zurück.
- **Vorschlag:** Metadaten-Änderungen in die Undo-Liste aufnehmen oder Strg+Z dafür klar abgrenzen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 8. Abbrechen nur für die erste Aktivität erreichbar (bestätigt)
- **Stand 18.10.:** **behoben** (Paket A) – aktuelle Stelle: `activity.rs:57-79`
- **Stelle:** `activity.rs:66-75`, Reihenfolge in `activities()`
- **Befund:** Das ✕ erscheint nur für `all.first()`. Nicht abbrechbare Aufgaben (Entfernen, KI-Maske, Look, Auto) stehen vorn. Läuft gleichzeitig ein Export oder HDR-Zusammenfügen, gibt es keine Abbrechen-Möglichkeit, auch nicht im Tooltip.
- **Vorschlag:** Abbrechbare Aktivitäten zuerst sortieren oder ein Popup mit allen Aktivitäten und je einem ✕.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): die Statusleiste zeigt zuerst eine abbrechbare Aufgabe mit ✕

### 9. Beenden-Dialog fragt nur bei abbrechbaren Aufgaben nach (bestätigt)
- **Stand 18.10.:** **behoben** (Paket A) – Dialog nutzt jetzt dialog::Dialog, filtert aber weiter auf cancel.is_some() – aktuelle Stelle: `app.rs:2438`
- **Stelle:** `app.rs:2391`
- **Befund:** Filtert auf `cancel.is_some()`. Wer während Entfernen, Look angleichen, Auto-Anpassen oder Indexieren schließt, verliert die Arbeit ohne Rückfrage.
- **Vorschlag:** Alle laufenden Aktivitäten berücksichtigen.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): Beenden fragt bei allen laufenden Aufgaben nach

### 10. „Alle Anpassungen zurücksetzen“ im Ebenenmodus aktiv
- **Stand 18.10.:** **behoben** (Paket A) – aktuelle Stelle: `commands.rs:108, chrome.rs:457`
- **Stelle:** `commands.rs:102` (`has_image`), Werkzeugleiste `chrome.rs:451`, `chrome.rs:24-30`
- **Befund:** Setzt das ganze Bild samt aller Ebenen zurück. Einfügen, Drehen, Auto-Anpassen, Look angleichen und Retusche sind auf einer Ebene dagegen gesperrt („Wirkt nur aufs ganze Bild“).
- **Vorschlag:** Auf einer Ebene sperren oder nur die Ebene zurücksetzen (Klärung, was gewünscht ist).
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): „Alle Anpassungen zurücksetzen“ ist auf einer Ebene deaktiviert (Befehl und Werkzeugleiste)

### 11. „In den Papierkorb“: außerhalb der Session wirkungslos, in der Session doppelt
- **Stand 18.10.:** **behoben** (Paket A) – doppelter Menüeintrag ist weg; Freigabe außerhalb der Session weiter has_selection – aktuelle Stelle: `commands.rs:173, session.rs:256-273`
- **Stelle:** `commands.rs` (`trash`, `has_selection`), `session.rs:252-268`, `session.rs:363`, Menü „Foto“ in `chrome.rs`
- **Befund:** Außerhalb einer Session aktiv, setzt aber nur „gibt es nur in einer Session“. Der Nachbarbefehl `to-selects` ist dort korrekt deaktiviert. In der Session steht der Eintrag zweimal im Menü: „In Papierkorb verschieben“ (mit fest eingetipptem Strg+Entf) und „In den Papierkorb“.
- **Vorschlag:** Freigabe `in_session`; einen der beiden Menüeinträge streichen.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): „In den Papierkorb“ außerhalb einer Session nur noch für Varianten aktiv

### 12. Esc im Bildfenster: umgekehrte Reihenfolge
- **Stand 18.10.:** **behoben** (Paket A) – aktuelle Stelle: `viewer_window.rs:84`
- **Stelle:** `viewer_window.rs:77-78`
- **Befund:** Im Bildfenster verlässt Esc zuerst das Vollbild, auch bei aktivem Zuschnitt oder Pinsel. Im Hauptfenster beendet Esc zuerst das Werkzeug (`app.rs:2378-2382`).
- **Vorschlag:** Reihenfolge wie im Hauptfenster.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): Esc im Bildfenster beendet zuerst das Werkzeug

### 13. F11 / Umschalt+F11 im Bildfenster fest verdrahtet
- **Stand 18.10.:** **behoben** – Tasten kommen aus der Befehlsliste – aktuelle Stelle: `viewer_window.rs:75-84`
- **Stelle:** `viewer_window.rs:77`, `:84`
- **Befund:** Umgeht die Befehle `fullscreen` und `viewer-window`. Nach dem Umbelegen wirkt dort weiter F11, die neue Taste schaltet das Vollbild des Hauptfensters.
- **Vorschlag:** Über die Befehlsliste auflösen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## B. Modale Werkzeuge

### 14. Esc bedeutet beim Zuschnitt „übernehmen“, bei Trapez-Linien „verwerfen“
- **Stand 18.10.:** **teilweise** – Esc verwirft jetzt den Zuschnitt; offen: jeder Zug am Rahmen ist ein eigener Undo-Schritt, Kommentar crop.rs:515 stimmt nicht – aktuelle Stelle: `crop.rs:302-357`
- **Stelle:** `crop.rs:279-289`, `crop.rs:491-496`, `crop.rs:215`, `develop_tools.rs:370`
- **Befund:** Der Zuschnitt wird live in `adjustments` geschrieben; Esc und Enter tun dasselbe, ein echtes Abbrechen gibt es nicht. Jeder Zug am Rahmen ist ein eigener Undo-Schritt. Der Kommentar „The crop is applied when the tool closes“ (`crop.rs:492`) stimmt nicht.
- **Vorschlag:** Einheitlich: Enter übernimmt, Esc verwirft (Zuschnitt bei Start merken).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 15. Übernehmen/Abbrechen-Knöpfe je Werkzeug verschieden
- **Stand 18.10.:** **teilweise** – einheitlich „Übernehmen“/„Abbrechen“, Zuschnitt hat Abbrechen; offen: Trapez doppelt, Geraderichten sofort, Entfernen mit „Letzten zurücknehmen“ – aktuelle Stelle: `develop_tools.rs:339-347, :460-467, crop.rs:213-223`
- **Stelle:** `develop_tools.rs:254-256`, `:366-372`, `crop.rs:208-217`, `remove.rs:204`
- **Befund:** Trapez: „Anwenden“/„Abbrechen“ doppelt (Panel und Overlay). Zuschnitt: nur „Übernehmen“ im Panel, kein Abbrechen, kein Overlay. Geraderichten: Linie wird beim Loslassen sofort angewendet. Entfernen: sofort angewendet, dazu eigenes „Letzten zurücknehmen“ neben Strg+Z.
- **Vorschlag:** Ein Muster für bestätigungspflichtige Werkzeuge festlegen (Panel und/oder Overlay, immer mit Abbrechen).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 16. Bildwechsel mitten im Werkzeug
- **Stand 18.10.:** **offen** – beim Werkzeugwechsel werden Linien jetzt übernommen, beim Bildwechsel weiter still verworfen – aktuelle Stelle: `app.rs:1267-1276, :1571-1573`
- **Stelle:** `app.rs:1245-1246` (`install`), `app.rs:1549-1551` (`make_primary`)
- **Befund:** Zuschnitt, Geraderichten/Trapez und Pipetten werden beendet; gesetzte Trapez-Linien gehen ohne Hinweis verloren. Pinsel, Verlauf, Zauberpinsel, Objekt und Entfernen bleiben aktiv, die Entfernen-Striche werden geleert.
- **Vorschlag:** Einheitliches Verhalten festlegen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 17. Tab-Wechsel beendet nur Ebenen- und Retusche-Werkzeuge
- **Stand 18.10.:** **offen** – aktuelle Stelle: `app.rs:2744-2751`
- **Stelle:** `app.rs:2677-2681`
- **Befund:** Zuschnitt, Geraderichten und Trapez bleiben im Bild aktiv, obwohl ihr Panel verschwindet. Beim Zuschnitt bleibt nur Enter, Esc oder C als Ausweg.
- **Vorschlag:** Beim Tab-Wechsel auch diese Werkzeuge beenden (oder Hinweis im Bild, siehe 19).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 18. Werkzeug starten wechselt den Tab nicht einheitlich
- **Stand 18.10.:** **offen** – aktuelle Stelle: `commands.rs:114-131, chrome.rs:431-443`
- **Stelle:** `commands.rs:108-125`, `chrome.rs:426-437`
- **Befund:** W und der Weißabgleich-Knopf öffnen den Farb-Tab. „Geraderichten“ aus der Palette öffnet den Objektiv-Tab, derselbe Knopf in der Werkzeugleiste nicht. C/„Zuschneiden“ wechselt nie den Tab.
- **Vorschlag:** Werkzeuge öffnen immer ihren Tab.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 19. Modus-Hinweise im Bild ungleich
- **Stand 18.10.:** **teilweise** – Zuschnitt und Trapez mit gleichem Hinweis; Geraderichten, Pipetten, Pinsel & Co. weiter ohne – aktuelle Stelle: `browser.rs:422-438`
- **Stelle:** `browser.rs:416-433`
- **Befund:** Zuschnitt zeigt „Esc beendet“, Trapez „Esc bricht ab“. Geraderichten und Pipetten nennen Esc nicht. Pinsel, Verlauf, Entfernen und Objekt haben keinen Hinweis.
- **Vorschlag:** Für alle Werkzeuge ein einheitlicher Hinweis.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 20. Esc verlässt Ansichtsmodi nicht
- **Stand 18.10.:** **offen** – aktuelle Stelle: `app.rs:2423-2429`
- **Stelle:** `app.rs:2376-2383`
- **Befund:** Vergleich, Vorher/Nachher, Druckvorschau, Softproof, Ausgabevorschau und 100 %-Zoom bleiben bei Esc bestehen. Esc wirkt nur auf Werkzeuge und Vollbild.
- **Vorschlag:** Klären, ob Esc auch Ansichtsmodi beenden soll.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## C. Verfügbarkeit („deaktivieren statt ausblenden“, Freigaben)

### 21. Maske bei gewähltem Hintergrund: Bedienelemente ausgeblendet
- **Stand 18.10.:** **offen** – aktuelle Stelle: `layers.rs:853-869, :899-910`
- **Stelle:** `layers.rs:853`, `:900`, `:909`
- **Befund:** Füllen, Leeren, Invertieren, „Maske anzeigen“ und der ganze Abschnitt „Eingrenzen“ fehlen; „Auswahl entfernen“ erscheint nur mit KI-Maske. Direkt darüber wird „Deckkraft“ korrekt deaktiviert.
- **Vorschlag:** Mit `add_enabled_ui(false)` anzeigen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 22. Abschnitte „nur ganzes Bild“ sehen zweierlei aus
- **Stand 18.10.:** **offen** – aktuelle Stelle: `widgets.rs:120-123 vs. :170-183`
- **Stelle:** `widgets.rs:91-94` (`background_only`) vs. `widgets.rs:142-153`
- **Befund:** „Automatisch anpassen“ und „Look angleichen“ bleiben im Ebenenmodus aufgeklappt und werden nur grau. Sonst gilt: eingeklappt mit Label „nur ganzes Bild“.
- **Vorschlag:** Auf das Mehrheitsmuster umstellen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 23. „Qualität“ im Ausgaberezept ausgeblendet
- **Stand 18.10.:** **offen** – aktuelle Stelle: `tools.rs:484`
- **Stelle:** `tools.rs:465`
- **Befund:** Bei Formaten ohne Qualität fehlt der Regler. Zehn Zeilen darunter steht „Options without effect are shown, but disabled“ und wird für Skalieren/KI-Vergrößern so umgesetzt.
- **Vorschlag:** Deaktivieren statt ausblenden.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 24. „Hochformat“ beim Zuschnitt ausgeblendet
- **Stand 18.10.:** **offen** – aktuelle Stelle: `develop_tools.rs:361`
- **Stelle:** `develop_tools.rs:272`
- **Befund:** Bei Seitenverhältnis „Frei“ ausgeblendet statt deaktiviert.
- **Vorschlag:** Deaktivieren.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 25. Retusche: Abschnitte und Punkteinstellungen ausgeblendet
- **Stand 18.10.:** **offen** – aktuelle Stelle: `remove.rs:190-195, layers.rs:1022-1045`
- **Stelle:** `remove.rs:190`, `remove.rs:195`, `layers.rs:1021`
- **Befund:** Der Entfernen-Abschnitt fehlt, solange das Werkzeug nicht aktiv ist und nichts entfernt wurde. Seine Pinselregler stehen in einem eigenen Abschnitt, bei Reparieren/Klonen unter den Werkzeugkacheln. Art, Deckkraft, Weichheit, „Neue Quelle“ und „Löschen“ fehlen, solange kein Punkt gewählt ist.
- **Vorschlag:** Deaktiviert anzeigen; Pinselregler an einheitlichem Ort.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 26. KI-Entrauschen bei HDR-DNG bedienbar, aber wirkungslos
- **Stand 18.10.:** **offen** – aktuelle Stelle: `denoise.rs:207-216`
- **Stelle:** `denoise.rs:212-215`
- **Befund:** Regler aktiv, darunter nur „nicht verfügbar“.
- **Vorschlag:** Regler deaktivieren.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 27. Trapez-„Stärke“ immer aktiv
- **Stand 18.10.:** **offen** – aktuelle Stelle: `develop_tools.rs:475`
- **Stelle:** `develop_tools.rs:380`
- **Befund:** Aktiv, auch wenn Vertikal und Horizontal auf 0 stehen. Bei Schärfen, Glow, Körnung usw. wird alles Formende ohne Stärke deaktiviert.
- **Vorschlag:** An Vertikal ≠ 0 oder Horizontal ≠ 0 koppeln.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 28. Laufende Aktion: drei verschiedene Verhalten
- **Stand 18.10.:** **behoben** (Paket A) – aktuelle Stelle: `commands.rs:120, look.rs:83-98, app.rs:1814, layers.rs:996, :1086`
- **Stelle:** `commands.rs` (`auto_progress.is_none()`), `look.rs:83-98`, `app.rs:1765`, `layers.rs:994`, `layers.rs:1084`
- **Befund:** Auto-Anpassen und Look angleichen sind während des Laufs deaktiviert (mit Hover-Grund). Exportieren bleibt aktiv und meldet erst beim Klick „Es läuft bereits ein Export“. „Staub und Flecken finden“ ist im Panel deaktiviert, als Befehl aktiv und ignoriert den Klick still.
- **Vorschlag:** Überall deaktivieren mit Hover-Grund.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): Export während eines Exports und „Staub und Flecken finden“ während der Suche deaktiviert

### 29. Exportieren mit drei verschiedenen Freigaben
- **Stand 18.10.:** **behoben** (Paket A) – aktuelle Stelle: `commands.rs:101, chrome.rs:466, tools.rs:599`
- **Stelle:** Befehl (`has_selection`), Werkzeugleiste `chrome.rs` (`n > 0`), Ausgabe-Tab `tools.rs` (`n > 0 && k > 0`)
- **Befund:** Ohne aktives Rezept bleiben Werkzeugleiste und Befehl aktiv und melden nur „Kein Ausgaberezept aktiviert“.
- **Vorschlag:** Einheitlich `n > 0 && k > 0` mit Hover-Grund.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): Export überall gleich freigegeben (Bild, aktives Rezept, kein laufender Export), Grund im Tooltip

### 30. Weitere Freigaben, die zwischen Befehl und Oberfläche abweichen
- **Stand 18.10.:** **behoben** (Paket A) – im Session-Panel ist „Papierkorb leeren“ korrekt deaktiviert, nur das Datei-Menü weicht ab – aktuelle Stelle: `chrome.rs:470, commands.rs:197/225/232, session.rs:397`
- **Stelle:** `chrome.rs:462`, `commands.rs:226`, `softproof.rs:102`, `print_preview.rs:168`, `commands.rs:182`, `layers.rs:1133`, `session.rs:392`, `:572`
- **Befund:** Vollbild: Befehl immer, Werkzeugleiste nur mit Bild. Softproof/Druckvorschau: Befehle brauchen ein Bild, die Checkboxen nicht. `delete-spot` ist immer aktiv, tut ohne gewählten Punkt nichts und holt Entf/Rück immer ab. „Papierkorb leeren …“ ist auch bei leerem Papierkorb aktiv.
- **Vorschlag:** Freigaben angleichen.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): Vollbild nur mit Bild; Softproof/Druckvorschau-Checkbox ohne Bild deaktiviert; Entf nur mit gewähltem Punkt/Verlauf; „Papierkorb leeren …“ im Datei-Menü nur bei vollem Papierkorb

### 31. Entf wirkt nur auf Retuschepunkte und Verläufe
- **Stand 18.10.:** **offen** – aktuelle Stelle: `commands.rs:197, layers.rs:1135`
- **Stelle:** `commands.rs:182`, `layers.rs:1133`
- **Befund:** Eine gewählte Ebene, ein Entfernen-Bereich oder ein Bild im Raster lässt sich mit Entf nicht löschen (im Raster braucht es Strg+Entf).
- **Vorschlag:** Klären, ob Entf kontextabhängig löschen soll.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## D. Rückmeldung bei langen Aufgaben und Fehlern

### 32. Abbrechen uneinheitlich angeboten
- **Stand 18.10.:** **offen** – aktuelle Stelle: `activity.rs:24-56`
- **Stelle:** `activity.rs:26-55`, `update.rs:240`, Fortschrittsbalken in `tools.rs` (Ausgabe), `denoise.rs`, `depth.rs`
- **Befund:** ✕ in der Statusleiste: Export und alle `start_job`-Jobs. Eigener Abbrechen-Knopf: nur Update. Nicht abbrechbar: Entfernen, Masken-KI, Look angleichen, Auto-Anpassen, Indexieren, Objektivprofile, Fleckensuche. Fortschrittsbalken im Panel haben kein Abbrechen.
- **Vorschlag:** Abbrechen überall dort, wo ein Fortschrittsbalken steht.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 33. Abgebrochene Jobs als Fehler gemeldet
- **Stand 18.10.:** **behoben** (Paket A) – aktuelle Stelle: `jobs.rs:46, :88`
- **Stelle:** `jobs.rs:47`, `jobs.rs:87`
- **Befund:** „HDR zusammenfügen fehlgeschlagen: abgebrochen“ (betrifft Merge, Gesichter). Sonst: „Export abgebrochen (n von m fertig)“, „Update abgebrochen“.
- **Vorschlag:** Abbruch als eigenen Fall melden: „… abgebrochen“.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): abgebrochene Jobs melden „… abgebrochen“ statt „fehlgeschlagen“

### 34. Fehler landen in vier verschiedenen Kanälen
- **Stand 18.10.:** **offen** – aktuelle Stelle: `app.rs:2124-2130, library.rs:551/739`
- **Stelle:** `denoise.rs`, `depth.rs:379`, `app.rs:2092`, `app.rs:2095`, `app.rs:2174`, `app.rs:2177`, `update.rs:106`, `library.rs` (`styles::delete_in`, `delete_album`), `layers.rs:1112`
- **Befund:** Meist Statuszeile (gedimmt, ohne Fehlerfarbe, wird überschrieben). Entrauschen und Tiefe zusätzlich rot im Panel. Export: Fehler nur im Export-Log, Statuszeile meldet „Export abgeschlossen (n)“ ohne Fehlerzahl; beim Import steht „n Fehler“ dabei. Still bzw. nur im Log: Indexierungsfehler außerhalb des Imports, Update-Prüfung, Stil löschen, Album löschen, verworfene Fleckensuche nach Bildwechsel.
- **Vorschlag:** Einheitlicher Fehlerkanal (Statuszeile in Fehlerfarbe, länger sichtbar), Fehlerzahl auch beim Export.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 35. Fleckensuche an der Aktivitäts-Infrastruktur vorbei
- **Stand 18.10.:** **offen** – aktuelle Stelle: `layers.rs:1084-1132`
- **Stelle:** `layers.rs:1092`
- **Befund:** Eigener Thread, nicht in `activities()`, nur Spinner im Panel, kein Fortschritt in der Statusleiste.
- **Vorschlag:** Über `activities()` laufen lassen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## E. Löschen, Bestätigung, Zurücksetzen

### 36. Bestätigung und Undo beim Löschen ohne einheitliches Prinzip
- **Stand 18.10.:** **offen** – Bestätigungen laufen jetzt über dialog::Dialog; Schnappschuss, Stil, Album, Rezept, Look-Referenz und Stichwort weiter ohne Bestätigung
- **Stelle:** `variants.rs:135`, `session.rs:568`, `culling.rs`, `history.rs:44`, `:106-107`, `side_column.rs`, `style_grid.rs:131-136`, `library.rs:502`, `:551`, `:735-736`, `tools.rs:325`, `:429`, `look.rs:113`
- **Befund:**

  | Aktion | Bestätigung | Rückgängig |
  |---|---|---|
  | Variante löschen, Papierkorb leeren, Personen zusammenführen | ja | nein |
  | Alles zurücksetzen, Ebene löschen, Retusche „Alle löschen“, Entfernen „Alle“ | nein | ja |
  | Schnappschuss löschen | nein | nein |
  | Eigenen Stil löschen (Datei weg) | nein | nein |
  | Album löschen | nein | nein |
  | Ausgaberezept entfernen | nein | nein |
  | Look-Referenz entfernen | nein | nein |
  | Stichwort per Chip-Klick entfernen | nein | nein |
- **Vorschlag:** Regel: Was nicht rückgängig zu machen ist, wird bestätigt (mindestens Stil, Album, Schnappschuss).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 37. Reset im Abschnittskopf deckt nicht alle Regler ab
- **Stand 18.10.:** **offen** – aktuelle Stelle: `develop_tools.rs:367-370, :402-407`
- **Stelle:** `develop_tools.rs:313`, `develop_tools.rs:278`, `widgets.rs:225`
- **Befund:** „Drehen & Spiegeln“ setzt die 90°-Drehung nicht zurück (`Tool::Rotation` enthält sie). „Zuschnitt“ setzt Seitenverhältnis und Hochformat nicht zurück (in `Tool::Crop` enthalten).
- **Vorschlag:** Reset umfasst genau das, was `Tool::copy` umfasst.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 38. Abschnitte ohne Reset im Kopf, Inline-Knöpfe mit drei Namen
- **Stand 18.10.:** **offen**
- **Stelle:** `tools.rs:195`, `look.rs:279`, Maske, Reparieren & Klonen, `remove.rs:207`, Ausgabe-Tab
- **Befund:** Basiskurve, Look angleichen, Maske, Reparieren & Klonen und Entfernen haben kein ↺ im Kopf; stattdessen „Leeren“, „Alle löschen“, „Alle“. Rezept und Wasserzeichen ohne Reset.
- **Vorschlag:** ↺ im Kopf, wo sinnvoll; Inline-Knöpfe einheitlich „Alle löschen“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## F. Leerzustände, Seitenleiste

### 39. Ohne Bild verhalten sich Tabs unterschiedlich
- **Stand 18.10.:** **offen** – aktuelle Stelle: `library.rs:629-702, tools.rs:380-383`
- **Stelle:** `library.rs:626ff` (Stile), `tools.rs:360` (Info)
- **Befund:** Mehrheit: `widgets::no_image()`. Stile: kein Leerzustand, das Raster fällt still auf die Liste zurück; unter „Neuer Stil“ fehlen Werkzeugauswahl und Speichern ohne Erklärung. Info zeigt „Kein Bild ausgewählt“, obwohl Bewertungen und Labels auf die Rasterauswahl wirken.
- **Vorschlag:** Stile: `no_image()` bzw. Hinweis; Info: Metadaten für die Auswahl bearbeitbar machen oder Hinweis anpassen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 40. Schnappschüsse: Seitenleiste weicht vom Verlauf-Tab ab
- **Stand 18.10.:** **offen** – aktuelle Stelle: `side_column.rs:202-242`
- **Stelle:** `side_column.rs:210-228` vs. `history.rs:77-110`
- **Befund:** Seitenleiste: Tooltip „Anwenden“ statt „Klick: anwenden · Doppelklick: umbenennen“, Umbenennen fehlt, „+ Schnappschuss“ unten statt oben, kein Leerhinweis. Der Tooltip „unter einem Namen sichern“ verspricht eine Namensabfrage, `add_snapshot(None)` fragt aber nicht. Der Verlauf wird in der Seitenleiste bei höchstens einem Schritt ganz ausgeblendet, der Tab zeigt „Geöffnet“.
- **Vorschlag:** Seitenleiste an den Tab angleichen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## G. Dialoge und Inline-Bearbeitung

### 41. Enter bestätigt nur in zwei Dialogen
- **Stand 18.10.:** **offen** – dialog.rs behandelt Enter nicht; weiter nur Umbenennen und Neue Session
- **Stelle:** `rename.rs:124`, `session.rs:516` (ja); `merge.rs`, `stacks.rs:309`, `culling.rs:364`, `print_preview.rs:257`, Variante löschen, Papierkorb leeren (nein)
- **Befund:** Überall sonst braucht es die Maus.
- **Vorschlag:** Enter löst überall den Hauptknopf aus (bei destruktiven Dialogen klären).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 42. Esc beim Inline-Umbenennen: verwerfen oder speichern?
- **Stand 18.10.:** **offen** – aktuelle Stelle: `layers.rs:1223, history.rs:91, library.rs:480`
- **Stelle:** `layers.rs:1220-1223`, `history.rs:96-98`, `library.rs:479-481`
- **Befund:** Ebene: Esc stellt den alten Namen her. Schnappschuss und Album speichern bei `lost_focus()`, also auch bei Esc. Einstieg: Ebene per Doppelklick oder Klick auf gewählten Namen, Schnappschuss per Doppelklick, Album nur per Kontextmenü.
- **Vorschlag:** Esc verwirft überall; Doppelklick startet überall das Umbenennen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 43. Kein gemeinsamer Dialog-Helfer
- **Stand 18.10.:** **teilweise** – gemeinsamer Helfer dialog::Dialog existiert (Titel mit Icon, Footer, primary/danger/secondary); offen: Enter/Esc im Helfer – aktuelle Stelle: `dialog.rs`
- **Stelle:** elf Modals; Ausreißer `app.rs:2408` (Beenden, 16 pt, ohne Icon), `print_preview.rs:235` (ohne Icon)
- **Befund:** Titel, Breite und Knopfleiste werden jeweils von Hand nachgebaut.
- **Vorschlag:** Helfer in `widgets.rs` (Titel mit Icon, Knopfleiste, Enter/Esc).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 44. Knopfreihenfolge in zwei Dialogen vertauscht
- **Stand 18.10.:** **teilweise** – Reihenfolge korrigiert; „Abbrechen und beenden“ heißt noch so und ist nicht danger – aktuelle Stelle: `app.rs:2465-2472`
- **Stelle:** `print_preview.rs:256-262`, `app.rs:2418-2425`
- **Befund:** Mehrheit: rechtsbündig, Bestätigen rechts außen. Diese beiden nutzen `ui.horizontal`, die bestätigende bzw. zerstörerische Aktion steht links, ohne Mindestgröße. „Abbrechen und beenden“ ist missverständlich, weil „Abbrechen“ sonst „Dialog schließen“ heißt; rote Füllung fehlt.
- **Vorschlag:** Rechtsbündig; „Aufgaben abbrechen und beenden“ o. ä. in Danger-Farbe.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 45. Hauptknopf nur manchmal hervorgehoben
- **Stand 18.10.:** **teilweise** – Hauptknopf überall in Akzentfarbe; offen: „Personen zusammenführen“ (nicht rückgängig) ist primary statt danger
- **Stelle:** `culling.rs:364`, `session.rs:544-550` (Akzent); `merge.rs:132`, `stacks.rs:309`, `rename.rs:124`, `update.rs:251` (neutral)
- **Befund:** Akzentfarbe nur in zwei Dialogen; Rot für Destruktives nur in `variants.rs` und `session.rs`.
- **Vorschlag:** Hauptknopf immer Akzent, destruktiv immer Danger.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 46. Kopieren-Dialog und Tastenkürzel sind `egui::Window` statt Modal
- **Stand 18.10.:** **behoben** – beide jetzt Modal mit Footer – aktuelle Stelle: `clipboard.rs:129-153, shortcut_editor.rs:89-181`
- **Stelle:** `clipboard.rs:128-133`, `shortcut_editor.rs:89-94`
- **Befund:** Schließen per Titel-X, kein Abdunkeln, keine Knopfleiste. Im Kopieren-Dialog fehlt „Abbrechen“, die Aktionen stehen links. Hängt mit 2 und 3 zusammen.
- **Vorschlag:** Als Modal umbauen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## H. Maus und Kontextmenüs

### 47. Kontextmenüs lückenhaft
- **Stand 18.10.:** **offen** – aktuelle Stelle: `browser.rs:940-945`
- **Stelle:** `browser.rs:882`, `compare.rs:323`, `layers.rs:1145ff`
- **Befund:** Vorhanden bei Vorschaubildern, Alben, Session-Ordnern, Favoriten, Stilen, Personen, Vergleich. Keines bei Ebenenliste, Schnappschüssen, Look-Referenzen, Retuschepunkten. Das Vorschaubild-Menü bietet weder Bewertung, Label, Umbenennen, Exportieren noch Anpassungen kopieren/einfügen.
- **Vorschlag:** Kontextmenüs für Listen ergänzen; Vorschaubild-Menü um die Foto-Befehle erweitern.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## I. Tastenkürzel-System umgangen

### 48. Fest eingetippte Kürzel in Menüs, Tooltips und Kacheln
- **Stand 18.10.:** **teilweise** – behoben in Menüs (Bewertung, Labels, Hilfslinien, Stapel, Varianten, Session), Willkommen, Umschalt+A/Strg+L, Ebenen-Kacheln; offen: „Enter“/„Esc“-Tooltips, „Vorschau mit P“, clipboard.rs:146, layers.rs:991
- **Stelle:**
  - `chrome.rs:216` („0–5“), `chrome.rs:222-229` („6–9“), `chrome.rs:356` („O“)
  - `stacks.rs:276-284` („Strg+G“, „Strg+Umschalt+G“)
  - `variants.rs:117`, `:120` („F2“, „F3“)
  - `session.rs:341`, `:363`, `:374`, `:377` („Strg+J“, „Strg+Entf“, „Strg+N“, „Strg+Umschalt+O“)
  - `develop_tools.rs:255`, `:367`, `:370`, `crop.rs:213`, `:215` („Enter“, „Esc“)
  - `develop_tools.rs:325`, `:332` („(Umschalt+A)“), `develop_tools.rs:740` („(Strg+L)“)
  - `layers.rs:55-65` („Pinsel (B)“, „Objekt (Umschalt+J)“ …), `layers.rs:1452`
  - `tools.rs:570` („Vorschau mit P“), `browser.rs:949`, `:951`, `:972-981`, `clipboard.rs:151`
- **Befund:** Nach dem Umbelegen im Kürzel-Editor zeigen diese Stellen falsche Tasten. Widerspricht dem Anspruch in `commands.rs:1-3`. Die Mehrheit nutzt `commands::hint(…)`.
- **Vorschlag:** Überall `commands::hint` / `command_item`.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## J. Begriffe und Texte

### 49. Englische Reste
- **Stand 18.10.:** **offen** – alles noch da; neu „Farbsaum Lila/Grün“ direkt unter „CA Rot/Cyan“ spricht zusätzlich für „Farbsaum Rot/Cyan“
- **Stelle/Befund:**
  - „Focus Stacking …“ `commands.rs:144`, `merge.rs:83`, README → „Fokus-Stacking …“ oder „Schärfentiefe erweitern …“
  - „Fisheye“ `develop_tools.rs:236-237` → „Fischauge“
  - „Master“ `develop_tools.rs:695`, `layers.rs:1625` → „Gesamt“
  - „Keywords“/„Keyword“ `tools.rs:330`, `:333`, `:380`, `palette.rs:110`, `library.rs:421`, `:430`, `chrome.rs:249`, README → „Stichwörter“
  - „Release-Seite öffnen“ `update.rs:254` → „Download-Seite öffnen“
  - „Split-Toning“ im Tooltip `look.rs:330` → streichen
  - „SPEED EDIT“ `browser.rs:565`, `:579` → „Schnellbearbeitung“
  - „CA Rot/Cyan“, „CA Blau/Gelb“ `develop_tools.rs:238-239` → „Farbsaum Rot/Cyan“ usw.
- **Vorschlag:** wie angegeben.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 50. Grenzfälle: bleiben diese Begriffe?
- **Stand 18.10.:** **offen** – aktuelle Stelle: `README.md:208, :754`
- **Befund:** „Session“, „Farb-Label“, „Look“, „Softproof“, „Navigator“, „Update(s)“. Alternativen: „Sitzung“, „Farbmarkierung“, „Aktualisierungen“. README nutzt außerdem „Preset“ (`README.md:208`, `:744`), die Oberfläche „Stil“/„Stilpinsel“.
- **Vorschlag:** Je Begriff festlegen; README auf „Stil“ umstellen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 51. „Auto“ vs. „Automatisch“, Symbol uneinheitlich
- **Stand 18.10.:** **offen** – aktuelle Stelle: `tools.rs:257/288, develop_tools.rs:384/417`
- **Stelle:** `tools.rs:235`, `:266`, `develop_tools.rs:295` („Auto“, Zauberstab); `develop_tools.rs:326` („Automatisch“, Perspektive-Symbol)
- **Befund:** Palette und Menü sagen „Weißabgleich automatisch“ / „Automatisch anpassen“.
- **Vorschlag:** Ein Wort (z. B. „Automatisch“) und ein Symbol (Zauberstab).
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 52. „Automatisch geraderichten“ ist doppeldeutig
- **Stand 18.10.:** **offen** – aktuelle Stelle: `commands.rs:121`
- **Stelle:** `commands.rs:116`, `crop.rs:70`, `develop_tools.rs:295`, `:326`
- **Befund:** Der Palettenbefehl (Umschalt+A) korrigiert die Perspektive; „Auto“ unter Geraderichten richtet nur den Horizont aus.
- **Vorschlag:** „Perspektive automatisch korrigieren“ bzw. „Horizont automatisch“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 53. Senkrecht/waagerecht vs. vertikal/horizontal (nicht committeter Stand)
- **Stand 18.10.:** **offen** – aktuelle Stelle: `develop_tools.rs:398-472`
- **Stelle:** `develop_tools.rs:309-310`, `:332-354`, `:377-378`
- **Befund:** Auto-Menü: „Senkrechte und waagerechte Kanten“, Linien-Menü: „Beides“; Regler: „Vertikal“/„Horizontal“; Checkboxen „Horizontal spiegeln“/„Vertikal“. Auto-Menü mit Knöpfen, Linien-Menü mit Radios.
- **Vorschlag:** Ein Begriffspaar durchgehend; beide Menüs gleich aufbauen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 54. Befehlssuche hat fünf Namen
- **Stand 18.10.:** **offen** – zusätzlich Platzhalter „Befehl suchen“ im Kürzel-Editor (shortcut_editor.rs:94)
- **Stelle:** `chrome.rs:372`, `chrome.rs:117`, `chrome.rs:592`, `palette.rs:228`, `shortcut_editor.rs:38`, `commands.rs:262`
- **Befund:** „Befehl suchen …“, „Befehle, Werkzeuge und Einstellungen suchen“, „Werkzeug oder Einstellung suchen“, „Funktion oder Werkzeug suchen …“, „Befehl oder Werkzeug suchen“, „Befehlssuche“.
- **Vorschlag:** „Befehlssuche“; Platzhalter/Tooltip „Befehl, Werkzeug oder Einstellung suchen“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 55. „Anpassungen einfügen“ heißt mal „anwenden“
- **Stand 18.10.:** **offen** – aktuelle Stelle: `commands.rs:107, chrome.rs:207/453`
- **Stelle:** `commands.rs:101`, `chrome.rs:447`, `chrome.rs:201`, `clipboard.rs:151`
- **Befund:** „Anpassungen auf Auswahl anwenden“ / „Anpassungen anwenden“; Gegenstück ist „Anpassungen kopieren“.
- **Vorschlag:** „Anpassungen einfügen“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 56. „Übernehmen“ vs. „Anwenden“ für dieselbe Enter-Aktion
- **Stand 18.10.:** **behoben** – überall „Übernehmen“ – aktuelle Stelle: `develop_tools.rs:341/462, crop.rs:216`
- **Stelle:** `develop_tools.rs:255` („Übernehmen“), `develop_tools.rs:367`, `crop.rs:211` („Anwenden“), `commands.rs:110`, `chrome.rs:425`
- **Befund:** Zuschnitt und Trapez-Linien bestätigen mit unterschiedlichen Wörtern.
- **Vorschlag:** „Übernehmen“; „Anwenden“ bleibt für Stile, Profile, Schnappschüsse.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 57. Weißabgleich-Pipette hat drei Namen
- **Stand 18.10.:** **offen** – aktuelle Stelle: `commands.rs:126, chrome.rs:277/439`
- **Stelle:** `commands.rs:121`, `tools.rs:262`, `chrome.rs:272`, `chrome.rs:433`
- **Befund:** „Weißabgleich-Pipette“, „Weißabgleich aufnehmen“, „Weißabgleich“.
- **Vorschlag:** „Weißabgleich-Pipette“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 58. „Auswahl“ hat zwei Bedeutungen
- **Stand 18.10.:** **offen** – aktuelle Stelle: `commands.rs:172, browser.rs:1017`
- **Stelle:** `commands.rs:166`, `browser.rs:951`, `library.rs:67`, `session.rs:455`
- **Befund:** Markierte Bildmenge und Session-Ordner „Selects“ („Nach „Auswahl“ verschieben“). „Session-Favoriten“ existiert schon.
- **Vorschlag:** Ordner umbenennen (z. B. „Ausgewählt“, „Treffer“) – Klärung nötig.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 59. „Tiefen“ und „Schatten“ für denselben Regler
- **Stand 18.10.:** **offen** – aktuelle Stelle: `develop_tools.rs:851, look.rs:330`
- **Stelle:** `tools.rs:93`, `sk-core/src/history.rs:51`, `develop_tools.rs:750`
- **Befund:** Überall „Tiefen“, nur die Checkbox bei „Automatisch anpassen“ sagt „Lichter, Schatten, Weiß und Schwarz“. Außerdem „Mitten“ (`look.rs:330`) vs. „Mitteltöne“ (`develop_tools.rs:697`).
- **Vorschlag:** „Lichter, Tiefen, Weiß und Schwarz“; „Mitteltöne“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 60. Werkzeugnamen in Palette und Kacheln
- **Stand 18.10.:** **offen** – aktuelle Stelle: `commands.rs:177-180, layers.rs:72-81`
- **Stelle:** `commands.rs:153-158`, `layers.rs:72-81`
- **Befund:** „Linearer Verlauf“ vs. „Linear“, „Objekt auswählen“ vs. „Objekt“, Kachel „Farbe wählen“ während sonst „aufnehmen“ gilt.
- **Vorschlag:** Kacheln kurz, Tooltip mit Palettennamen; „Farbe aufnehmen“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 61. „Rauschen“ vs. „Rauschreduzierung“
- **Stand 18.10.:** **offen** – zusätzlich Palettenbefehl „KI-Entrauschen“
- **Stelle:** `denoise.rs:17`, `sk-core/src/history.rs:134-136`
- **Befund:** Abschnitt „Rauschen“, Verlauf „Rauschreduzierung“ / „KI-Entrauschen“.
- **Vorschlag:** Einen Begriff wählen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 62. Importieren unterschiedlich formuliert
- **Stand 18.10.:** **offen** – neu dazu library.rs:399 „Ordner importieren“ – aktuelle Stelle: `chrome.rs:71/189/405, palette.rs:146`
- **Stelle:** `chrome.rs:183`, `palette.rs:146`, `chrome.rs:399`, `chrome.rs:70`
- **Befund:** „Ordner in Katalog importieren …“ / „In Session importieren …“ vs. „Ordner in den Katalog importieren“ / „Bilder in die Session importieren“.
- **Vorschlag:** „Ordner in den Katalog importieren …“, „Bilder in die Session importieren …“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 63. Menü und Palette driften auseinander
- **Stand 18.10.:** **offen**
- **Stelle:** `commands.rs:147` vs. `chrome.rs:277`; `commands.rs:91` vs. `chrome.rs:317`; `commands.rs:202` vs. `chrome.rs:363`
- **Befund:** „Referenzbild festlegen / entfernen“ vs. „Als Referenzbild festlegen“; „Vorher / Nachher“ vs. „Vorher anzeigen“; „Bild in eigenem Fenster“ vs. „… (Zweitmonitor)“.
- **Vorschlag:** Gleicher Wortanfang in Menü und Palette.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 64. „Werkzeug“ hat drei Bedeutungen
- **Stand 18.10.:** **offen**
- **Stelle:** `palette.rs:134`, `commands.rs:265`, `side_column.rs:192`, `layers.rs:1460`
- **Befund:** Tabs, Abschnitte und lokale Werkzeuge (Pinsel usw.) heißen alle „Werkzeug“.
- **Vorschlag:** Tabs „Bereich“, Abschnitte „Werkzeug“, Pinsel & Co. „Malwerkzeug“ – mindestens die Palettengruppe für Tabs umbenennen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 65. „Entfernen“ überladen
- **Stand 18.10.:** **offen**
- **Stelle:** `commands.rs:178`, `remove.rs:194`, `tools.rs:325`, `:429`, `develop_tools.rs:208`
- **Befund:** Name des KI-Retusche-Werkzeugs und zugleich allgemeiner Knopf (Stichwort, Rezept, Objektivprofil); bei Ebenen und Varianten „Löschen“.
- **Vorschlag:** Werkzeug „Objekt entfernen“; „Löschen“ wenn etwas weg ist, „Entfernen“ wenn nur eine Zuordnung aufgehoben wird.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 66. Fortschrittsmeldungen in drei Satzformen
- **Stand 18.10.:** **offen**
- **Stelle:** `layers.rs:762`, `browser.rs:87`, `:452`, `app.rs:1786`, `library.rs:162-163`, `develop_tools.rs:739`, `look.rs:172`, `activity.rs:30`, `develop_tools.rs:224`, `depth.rs:370`
- **Befund:** „Berechne Maske…“ / „Passe an …“ / „Maske wird berechnet“; mit und ohne Leerzeichen vor „…“.
- **Vorschlag:** Passiv mit „ …“: „Maske wird berechnet …“, „Wird geladen …“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 67. Zähler in drei Varianten
- **Stand 18.10.:** **offen**
- **Stelle:** `splash.rs:116`, `library.rs:408`, `activity.rs:45`, `:53`, `chrome.rs:659`, `app.rs:2103`
- **Befund:** „Indexiere Bilder … 3 / 10“, „Indexiere 3 / 10“, „Indexieren 3 / 10“, „(3 von 10)“.
- **Vorschlag:** „Indexieren: 3 von 10“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 68. „Bild(er)“ statt `widgets::image_count`
- **Stand 18.10.:** **offen** – weitere Stelle app.rs:2440
- **Stelle:** `library.rs:558`, `app.rs:1750`, `stacks.rs:171`
- **Befund:** Sonst überall „1 Bild“ / „3 Bilder“.
- **Vorschlag:** `image_count` nutzen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 69. Großschreibung nach Doppelpunkt
- **Stand 18.10.:** **offen**
- **Stelle:** `depth.rs:167`, `layers.rs:421`, `app.rs:2128`, `:2131`, `print_preview.rs:148` (groß); `look.rs:147`, `crop.rs:70` (klein)
- **Vorschlag:** Klein nach Doppelpunkt.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 70. Einzige Du-Anrede
- **Stand 18.10.:** **offen** – aktuelle Stelle: `print_preview.rs:237`
- **Stelle:** `print_preview.rs:237`
- **Befund:** „Halte eine Scheckkarte … und ändere die Größe …“; sonst überall Infinitiv.
- **Vorschlag:** „Eine Scheckkarte an den Bildschirm halten und die Größe ändern, bis …“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 71. Schlusspunkt in Tooltips
- **Stand 18.10.:** **offen**
- **Stelle:** `culling.rs:252`, `tools.rs:462`, `variants.rs:126`, `print_preview.rs:148`
- **Befund:** 88 Tooltips ohne Punkt, drei mit.
- **Vorschlag:** Ohne Schlusspunkt.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 72. „&“ vs. „und“, Schreibweisen
- **Stand 18.10.:** **offen**
- **Stelle:** `palette.rs:81`, `side_column.rs:322`, `tools.rs:415`, `chrome.rs:492`, `tools.rs:426`, `library.rs:514`
- **Befund:** „Drehen & Spiegeln“, „Objektiv & Geometrie“ … vs. „Lichter und Tiefen“. „Ausgaberezepte“ vs. „Ausgabe-Rezept“. „+ Rezept“ / „+ Album“ mit Pluszeichen vs. PLUS-Icon bei „Schnappschuss“ und „Neue Ebene“.
- **Vorschlag:** Abschnittstitel mit „&“, Sätze mit „und“; „Ausgaberezept“; überall PLUS-Icon.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## K. Wertanzeige an Reglern

### 73. Dezimaltrennzeichen gemischt
- **Stand 18.10.:** **offen**
- **Stelle:** `theme.rs` (kein `number_formatter`), `crop.rs:78`, `:138`, `:540`, `:553`, `look.rs:545`, `layers.rs:1635`, `browser.rs:35`, `:565`, `:579`
- **Befund:** Regler zeigen „0.50“, Statustexte bewusst „0,50“. Das Overlay „+1.5°“ und die Schnellbearbeitung nutzen den Punkt.
- **Vorschlag:** Globaler `style.number_formatter` mit Komma in `theme::install`.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 74. Schnellbearbeitung: Formatierung und Temperatur
- **Stand 18.10.:** **offen** – aktuelle Stelle: `browser.rs:601-635`
- **Stelle:** `browser.rs:562`, `:565`, `:576`, `:579`
- **Befund:** `{:+.2}` gibt auch Kontrast und Kelvin mit zwei Nachkommastellen aus („+12.00“, „+5600.00“). Auf Ebenen „Temperatur ±“, aufs ganze Bild Kelvin in 50er-Schritten.
- **Vorschlag:** Nachkommastellen je Regler wie im Panel.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 75. Deckkraft, Härte, Fluss, Toleranz 0..1 statt %
- **Stand 18.10.:** **offen**
- **Stelle:** `layers.rs:177-181`, `layers.rs:1495` vs. `layers.rs:1001`, `:1027-1035`, `:1164`, `tools.rs:567`
- **Befund:** Pinsel und Zauberpinsel 0..1 mit zwei Dezimalen; Retusche, Ebene, Wasserzeichen und Spot-Empfindlichkeit 0..100 %.
- **Vorschlag:** Überall 0..100 %.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 76. Weichheit, Breite, Bereich mit drei Skalen
- **Stand 18.10.:** **offen**
- **Stelle:** `layers.rs:920-922`, `:940-941`, `depth.rs:210-332`, `develop_tools.rs:618`
- **Befund:** 0..0.5 mit zwei Dezimalen, 0..60 ohne Einheit, 0..100 %. Tiefen-Regler, Luminanz Von/Bis 0..1, „Min. Chroma“ 0..0.2 mit drei Dezimalen.
- **Vorschlag:** Anzeige in %, intern umrechnen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 77. Einheit „°“ fehlt bei Winkeln teilweise
- **Stand 18.10.:** **teilweise** – Masken-Farbton hat jetzt „°“; offen: Geraderichten, Farbeditor, Hauttöne – aktuelle Stelle: `develop_tools.rs:375/99/780, layers.rs:941`
- **Stelle:** mit: `develop_tools.rs:149`, `:236`, `:616-617`; ohne: `develop_tools.rs:108`, `:286`, `:678`, `layers.rs:938-939`
- **Befund:** Geraderichten ±45 ohne „°“, obwohl das Overlay „+1.5°“ zeigt; Farbton im Farbeditor, Hauttöne und Masken-Farbton ohne.
- **Vorschlag:** Überall „°“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 78. Belichtung ohne „EV“
- **Stand 18.10.:** **offen** – aktuelle Stelle: `tools.rs:99, depth.rs:351`
- **Stelle:** `tools.rs:78`, `depth.rs:351`
- **Befund:** Kelvin hat „K“, DPI „dpi“, Belichtung keine Einheit.
- **Vorschlag:** „ EV“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 79. „Stärke“ mal mit, mal ohne %, verschiedene Bereiche
- **Stand 18.10.:** **offen**
- **Stelle:** mit %: `develop_tools.rs:380`, `:776`, `look.rs:396`; ohne: `develop_tools.rs:150`, `:499`, `:508`, `depth.rs:202`, `:282`; Schärfen 0..500 `develop_tools.rs:447`
- **Vorschlag:** Einheitlich „ %“ wo 0..100.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 80. Tönung ±150 mit einer Dezimale vs. ±100 ganzzahlig
- **Stand 18.10.:** **offen** – aktuelle Stelle: `tools.rs:278 vs. :253`
- **Stelle:** `tools.rs:256` vs. `tools.rs:231`
- **Befund:** Gleiche Bezeichnung, verschiedene Bereiche (ganzes Bild vs. Ebene).
- **Vorschlag:** Klären, ob angleichbar.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 81. Vorzeichen fehlt an bipolaren Reglern
- **Stand 18.10.:** **offen** – aktuelle Stelle: `widgets.rs:452`
- **Stelle:** `widgets.rs:380` (`slider_row_with`); Übersichten `layers.rs:1635`, `:1642`, `look.rs:545`
- **Befund:** Übersichten zeigen „+12“, Regler „12“.
- **Vorschlag:** Für Bereiche mit negativem Minimum `custom_formatter` mit `{:+}`.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 82. Fokusmasken-„Empfindlichkeit“ ist ein Standard-Slider
- **Stand 18.10.:** **offen** – heißt jetzt „Schärfemaske“ – aktuelle Stelle: `chrome.rs:340-345`
- **Stelle:** `chrome.rs:336-339`
- **Befund:** Andere Optik, kein Doppelklick-Reset, eigenes Mausrad-/Tastaturverhalten, Label per `add_space(24)`. Alle anderen ~100 Regler nutzen die gemeinsamen Helfer.
- **Vorschlag:** `compact_slider`.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

---

## L. Optik und Layout

### 83. Split-Buttons können umbrechen (nicht committeter Stand)
- **Stand 18.10.:** **behoben** – aktuelle Stelle: `develop_tools.rs:413-457`
- **Stelle:** `develop_tools.rs:324-363`, Panel-Minimum `app.rs:2726`
- **Befund:** „Automatisch“ + ▾ und „Linien ziehen“ + ▾ stehen in `horizontal_wrapped`; der Pfeil kann allein in die nächste Zeile rutschen. Vorbild `auto_section` (`develop_tools.rs:737-760`) nutzt `ui.horizontal`.
- **Vorschlag:** Jedes Paar in ein eigenes `horizontal` innerhalb des Wrap.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.)

### 84. Drehen-Zeile läuft über (nicht committeter Stand)
- **Stand 18.10.:** **behoben** – aktuelle Stelle: `develop_tools.rs:378-395`
- **Stelle:** `develop_tools.rs:289-305`, Warnung `widgets.rs:383`
- **Befund:** Vier Knöpfe (~295 px) in nicht umbrechendem `ui.horizontal`; bei minimaler Panelbreite wächst das Panel.
- **Vorschlag:** `horizontal_wrapped` wie bei der Perspektive.
- **Entscheidung:** [x] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** erledigt (18.10.): `horizontal_wrapped`

### 85. Gleiches Symbol für Zurücksetzen und Linksdrehung
- **Stand 18.10.:** **teilweise** – Tooltips da, Symbol wie beim Zurücksetzen – aktuelle Stelle: `develop_tools.rs:387/391`
- **Stelle:** `widgets.rs:224`, `develop_tools.rs:298`
- **Befund:** ↺ „90°“ nutzt denselben Codepoint wie der Reset-Knopf direkt darüber; beide 90°-Knöpfe ohne Tooltip zur Richtung.
- **Vorschlag:** Eigenes Drehsymbol, Tooltips „90° nach links/rechts drehen“.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:** Tooltips „90° nach links/rechts drehen“ ergänzt (18.10.); das Symbol ist noch offen

### 86. Rohe Phosphor-Codepoints statt `icon::*`
- **Stand 18.10.:** **offen** – jetzt 34 Stellen
- **Stelle:** 33 Stellen, u. a. `develop_tools.rs:255`, `:258`, `tools.rs:429`, `layers.rs` (13×), `library.rs` (12×)
- **Befund:** Nicht sichtbar, erschwert aber Konsistenz (siehe 85).
- **Vorschlag:** Auf `icon::*` umstellen.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 87. Löschen-Symbole und Umschalter-Stile gemischt
- **Stand 18.10.:** **offen** – neu: X_SQUARE bei „Variante löschen …“ (variants.rs:126)
- **Stelle:** `layers.rs:1267`, `tools.rs:429`, `tools.rs:262`, `depth.rs:206`, `develop_tools.rs:668`
- **Befund:** Löschen als MINUS (Ebene), TRASH (Schnappschuss), TRASH + „Entfernen“ (Rezept). Werkzeug-Umschalter mal `selectable_label` (rahmenlos, Pipetten), mal `Button::selected` (gerahmt, „Linien ziehen“, „Aufnehmen“).
- **Vorschlag:** TRASH für Löschen; ein Stil für Umschalter.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 88. Hinweis- und Leertexte in fünf Grautönen
- **Stand 18.10.:** **teilweise** – Dialoge einheitlich mit dialog::note – aber das ist ein weiterer eigener Stil neben widgets::hint; Panels weiter gemischt
- **Stelle:** `widgets::hint` (8×) vs. `.small().color(TEXT_FAINT)` (17×), `.weak()` (8×), `from_gray(150/160/170)`; z. B. `library.rs:451`, `:509`, `:681`, `session.rs:474`, `merge.rs:101`, `rename.rs:107`, `gpu.rs:63`, `update.rs:183`, `denoise.rs:256`, `culling.rs:432`
- **Befund:** „Noch keine Alben.“ sieht anders aus als „Noch keine Schnappschüsse.“
- **Vorschlag:** Überall `widgets::hint`.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**

### 89. Hart codierte Farben statt Theme-Tokens
- **Stand 18.10.:** **teilweise** – Löschen-Dialoge nutzen dialog::danger; offen: softproof.rs:142 und die Tokens GUIDE/HOVER_FILL – aktuelle Stelle: `softproof.rs:142, dialog.rs:176`
- **Stelle:** Danger `session.rs:540`, `:579`, `variants.rs:147`, `softproof.rs:142`; Hilfslinien-Gelb `crop.rs:198`, `:535`, `layers.rs:658-660`; Hover-Füllungen `side_column.rs:187`, `layers.rs:1184`, `chrome.rs:119`, `:542`, `:899-910`, `browser.rs:827-837`
- **Befund:** `from_rgb(230,110,100)` ist exakt `theme::DANGER`, wird aber nicht genutzt; Löschen-Knopf `from_rgb(180,60,50)` doppelt.
- **Vorschlag:** Tokens `DANGER`, `DANGER_FILL`, `GUIDE`, `HOVER_FILL` in `theme.rs`.
- **Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
- **Notiz:**
