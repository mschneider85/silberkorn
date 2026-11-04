# Ausgabe

Der Tab **Ausgabe** legt fest, wie aus der Bearbeitung fertige Dateien werden: Format, Größe, Ordner, Wasserzeichen und Metadaten. Dazu kommen Softproof und Druckvorschau, die nur die Anzeige betreffen, und das Drucken.

## Exportieren

1. Bilder auswählen.
2. **Exportieren** drücken: der blaue Knopf rechts in der Werkzeugleiste, **Datei → Exportieren**, der Knopf im Abschnitt **Exportieren** des Tabs **Ausgabe** oder {taste:export}.

Jedes ausgewählte Bild wird mit jedem aktivierten Ausgaberezept geschrieben, in einem Durchgang. Der Fortschritt steht in der Statusleiste und im Abschnitt **Exportieren**; ✕ bricht ab. Man kann währenddessen weiterarbeiten, nur ein zweiter Export wartet, bis der erste fertig ist.

Ohne eigene Einstellungen schreibt das Rezept „JPEG“ ein JPEG in voller Größe und sRGB in den Unterordner `Output` neben dem Bild. In einer Session landen die Dateien im Ordner **Ausgabe** der Session.

> Ist **Exportieren** ausgegraut, nennt der Tooltip den Grund, etwa „Kein Ausgaberezept aktiviert“.

## Ausgaberezepte

Ein Ausgaberezept ist eine gespeicherte Ausgabe-Einstellung, etwa „JPEG fürs Web“ und „TIFF für den Druck“. Der Abschnitt **Ausgaberezepte** listet alle Rezepte:

- Das Häkchen davor bestimmt, ob ein Rezept beim Exportieren verwendet wird. Mehrere Rezepte zugleich sind möglich.
- Ein Klick auf den Namen wählt das Rezept zum Bearbeiten.
- **+ Rezept** legt eine Kopie des gewählten an, **Löschen …** entfernt es.

### Rezept bearbeiten

- **Name** des Rezepts.
- **Format**: **JPEG**, **TIFF 8 Bit**, **TIFF 16 Bit**, **PNG**, **WebP** oder **AVIF**.
- **Farbraum**: sRGB für Web und Bildschirm, Adobe RGB, Display P3 oder ProPhoto für Druck und Weiterbearbeitung. AVIF gibt es nur in sRGB und Display P3.
- **Qualität**: für JPEG, WebP und AVIF; verlustfreie Formate haben keine.
- **Größe**: **Originalgröße** oder **Lange Kante**, **Kurze Kante**, **Breite**, **Höhe** in Pixeln bzw. **Prozent**. Vergrößert wird nie.
- **Ausgabeschärfung**: **Aus**, **Für Bildschirm** (dezent) oder **Für Druck** (kräftiger). Sie gleicht die Weichheit aus, die beim Verkleinern entsteht, und ist unabhängig vom Schärfen in der Entwicklung.
- **Ordner**: Zielordner; ein relativer Name wie `Output` liegt neben dem jeweiligen Bild.
- **Dateiname** und **Suffix**: siehe unten.

### Dateiname

Leer bleibt der ursprüngliche Name. Sonst gelten dieselben Platzhalter wie beim [Umbenennen](bibliothek#Umbenennen), etwa `{Datum}_{Name}` oder `{Session}_{Zähler:3}`; der Zähler zählt die Bilder eines Durchgangs. Das **Suffix** wird angehängt, etwa `-web`. Varianten erhalten automatisch `-v2`, `-v3` usw.

## Wasserzeichen

Der Abschnitt **Wasserzeichen** gehört zum gewählten Rezept. **Wasserzeichen einfügen** schaltet es ein.

- **Text** (in **Weiß** oder **Schwarz**) oder **Logo** (**Logo wählen …**; ein PNG mit Transparenz ist am besten).
- **Position**: neun Stellen von **Oben links** bis **Unten rechts**.
- **Größe** und **Abstand** in Prozent der kurzen Bildkante, dazu **Deckkraft**.

Wie es im fertigen Bild aussieht, zeigt die [Ausgabevorschau](ausgabe#Ausgabevorschau).

## Metadaten

Ebenfalls je Rezept:

- **Aufnahmedaten übernehmen**: Kamera, Objektiv, Belichtung und Aufnahmedatum.
- **Fotograf** und **Copyright** werden in jede Datei geschrieben.

GPS-Daten werden nie übernommen. TIFF-Dateien erhalten keine Metadaten.

## Ausgabevorschau

**Ausgabevorschau** ({taste:proof}) zeigt das Bild so, wie das gewählte Ausgaberezept es schreibt: in der Zielgröße, mit Ausgabeschärfung, Wasserzeichen und Kompression. Oben im Bild stehen Format, Pixelmaße und Farbraum. Mit eingeschalteter **Farbumfang-Warnung** im Softproof markiert sie magenta, was der Farbraum der Datei beschneidet.

## Softproof

**Softproof** ({taste:softproof}) zeigt das Bild so, wie es in einem anderen Farbraum oder auf einem Drucker mit einem bestimmten Papier herauskommt. Eingestellt wird im Abschnitt **Softproof**:

- **Softproof anzeigen** schaltet ihn ein.
- **Profil**: **Wie Ausgaberezept** (der Farbraum des gewählten Rezepts), sRGB, Adobe RGB, Display P3 oder ein ICC-Profil eines Druckers bzw. Papiers. Weitere Profile lädt **ICC-Profil laden …**.
- **Priorität**: **Wahrnehmungsorientiert**, **Relativ farbmetrisch**, **Sättigung** oder **Absolut farbmetrisch**, dazu **Tiefenkompensierung**.
- **Papier und Druckfarbe simulieren** zeigt den Papierton und das tatsächliche Schwarz des Drucks.
- **Farbumfang-Warnung** färbt Farben magenta, die das Profil nicht wiedergeben kann.
- **Papierrand zeigen** legt Papier um das Bild.

Der Softproof ändert nur die Anzeige, nicht den Export.

## Druckvorschau

**Druckvorschau** ({taste:print-preview}) zeigt das Bild auf dem Papier, so wie es gedruckt wird, mit Rand, Rahmen, Schnittmarken und Beschriftung. Eingestellt wird im Abschnitt **Druckvorschau**:

- **Drucker** (nur unter Windows): voreingestellt der Standarddrucker von Windows.
- **Papierformat**: die Papiere, die der gewählte Drucker anbietet (unter Linux die üblichen Formate), oder **Eigenes (cm)**. Ein Format, das der Drucker nicht führt, steht gelb da.
- **Bildgröße**: **Ganzes Blatt** (so groß, wie Papier und Rand erlauben) oder ein festes Format wie **10 × 15 cm**, auch eigene Maße. Das Bild sitzt dann mittig auf dem Papier, etwa ein 10 × 15 auf A4; mit **Schnittmarken** lässt es sich genau ausschneiden. Der Rand ergibt sich dabei von selbst.
- **Ausrichtung**: **Hochformat** oder **Querformat**. Jede Seite beginnt so, wie ihr Bild liegt; die Knöpfe stellen nur die gezeigte Seite um, etwa ein Querformatbild auf ein hochkantes Blatt. Beim nächsten Öffnen der Druckleiste liegt wieder jede Seite wie ihr Bild. Mischt ein Druckauftrag Hoch- und Querformate, kommt jede Seite so aufs Papier, wie die Vorschau sie zeigt: Sie wird dafür gedreht an den Drucker geschickt.
- **Anpassung**: **Einpassen** (ganzes Bild, weiße Streifen, wo die Seitenverhältnisse abweichen) oder **Füllen** (randlos wie im Fotolabor, das Bild wird beschnitten). Die Liste sagt für das angezeigte Bild, wie viel weiß bleibt oder abgeschnitten wird. Mit fester Bildgröße gilt sie für diese: Ein 3:2-Bild auf 9 × 13 hat mit **Einpassen** oben und unten je 2 mm weiß, mit **Füllen** fehlen links und rechts je 3 mm. Die Schnittmarken sitzen dann am Format.

Oben im Bild steht die Auflösung auf dem Papier. Unter 200 ppi wird sie gelb, unter 150 ppi rot: Dort werden Details im Druck weich. Für die richtigen Farben den Softproof mit dem Profil des Druckers dazuschalten.

**Druckgröße** ({taste:print-zoom}) zeigt das Bild auf dem Bildschirm genau so groß, wie es gedruckt wird.

### Bildschirm kalibrieren

Damit die Druckgröße stimmt, muss Silberkorn die echte Größe des Bildschirms kennen. **Bildschirm kalibrieren …** zeigt ein Rechteck: eine Scheckkarte an den Bildschirm halten und die Größe ändern, bis beide genau gleich groß sind, dann **Übernehmen**.

## Druckseite

Der Abschnitt **Druckseite** legt fest, was außer dem Bild auf dem Blatt steht. Die Druckvorschau zeigt es mit.

- **Rand**: weißer Rand um das Bild auf allen Seiten, in mm (nur bei **Ganzes Blatt**). Bei **Füllen** füllt das Bild dann den Platz innerhalb des Randes.
- **Rahmen**: eine dünne schwarze Linie um das Bild, in mm; 0 heißt kein Rahmen.
- **Schnittmarken**: kurze Linien an den Ecken, außerhalb des Bildes, zum Zuschneiden, etwa wenn ein 13 × 18 auf A4 gedruckt wird. Sie brauchen einen Rand.
- **Beschriftung** unter dem Bild: **Dateiname**, **Aufnahmedaten** (Kamera, Brennweite, Blende, Zeit, ISO, Datum) oder beides.

Bild und Beschriftung stehen immer in der Mitte des Blattes.

## Drucken

**Drucken …** ({taste:print}, auch im Menü **Datei** und unten im Abschnitt **Drucken**) zeigt zuerst die Seite: Die Druckvorschau geht an, und unten im Bild erscheint eine Leiste mit Drucker, Papierformat, Bildgröße, Hoch- oder Querformat, Anpassung und **Einstellungen …**. Jedes ausgewählte Bild kommt auf eine eigene Seite; ist keines ausgewählt, das angezeigte. Bei mehreren Bildern blättern die Pfeile in der Leiste (oder die Pfeiltasten) durch die Seiten, die Auswahl bleibt.

**Einstellungen …** in der Leiste öffnet die Einstellungen des Druckers, etwa Papiersorte, Qualität und Farbe; sie gelten, bis Silberkorn beendet wird. Papierformat und Ausrichtung kommen immer aus der Leiste. **Drucken** (Enter) schickt die Seiten an den Drucker. **Abbrechen** (Esc) schließt die Leiste.

Führt der Drucker das Papierformat nicht, steht es in der Leiste gelb. Nimmt er es auch als eigenes Format nicht an, druckt er auf sein Standardpapier, und die Seite wird dafür neu aufgeteilt. Die Statusleiste sagt das danach.

Im Abschnitt **Drucken**:

- **Druckanpassung**: **Helligkeit** und **Kontrast** nur für den Druck. Drucke wirken oft dunkler und flacher als der leuchtende Bildschirm; die Vorschau zeigt die Anpassung nicht.
- **Schärfung**: **Aus**, **Wenig**, **Normal** oder **Stark**, für **Glänzend** oder **Matt** (mattes Papier saugt die Tinte auf und braucht mehr). Sie gleicht die Weichheit des Drucks aus und ist unabhängig vom Schärfen in der Entwicklung.
- **Farben**: **Drucker steuert** (der Drucker bekommt sRGB und rechnet selbst um) oder ein ICC-Profil für Drucker und Papier. Mit Profil rechnet Silberkorn die Farben um, mit der gewählten **Priorität**, und schaltet die Farbsteuerung des Treibers ab.

> Mit einem eigenen Profil im Druckertreiber „Keine Farbkorrektur“ wählen, falls er das anbietet; sonst werden die Farben doppelt umgerechnet. Den Softproof mit demselben Profil einschalten, um vorher zu sehen, wie der Druck aussieht.

Gedruckt wird mit bis zu 360 ppi; den Rest erledigt der Druckertreiber. Drucken gibt es nur unter Windows.

### In Datei drucken

**In Datei …** (oder **Datei → In Datei drucken …**) speichert die Seiten statt sie zu drucken: als PDF mit allen Seiten oder als JPEG je Seite, mit 300 ppi und in der Papiergröße, etwa für ein Fotolabor oder einen Copyshop. Mit Druckerprofil sind die Farben schon umgerechnet und das Profil ist eingebettet.
