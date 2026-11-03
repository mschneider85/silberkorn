# Ausgabe

Der Tab **Ausgabe** legt fest, wie aus der Bearbeitung fertige Dateien werden: Format, Größe, Ordner, Wasserzeichen und Metadaten. Dazu kommen Softproof und Druckvorschau, die nur die Anzeige betreffen.

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

Silberkorn druckt nicht selbst, hilft aber, einen Druck vorzubereiten. **Druckvorschau** ({taste:print-preview}) zeigt das Bild auf dem Papier, eingestellt im Abschnitt **Druckvorschau**:

- **Papierformat** aus der Liste oder **Eigenes (cm)**.
- **Anpassung**: **Einpassen** (ganzes Bild, Rand, wo die Seitenverhältnisse abweichen) oder **Füllen** (randlos wie im Fotolabor, das Bild wird beschnitten).

Oben im Bild steht die Auflösung auf dem Papier. Unter 200 ppi wird sie gelb, unter 150 ppi rot: Dort werden Details im Druck weich. Für die richtigen Farben den Softproof mit dem Profil des Druckers dazuschalten.

**Druckgröße** ({taste:print-zoom}) zeigt das Bild auf dem Bildschirm genau so groß, wie es gedruckt wird.

### Bildschirm kalibrieren

Damit die Druckgröße stimmt, muss Silberkorn die echte Größe des Bildschirms kennen. **Bildschirm kalibrieren …** zeigt ein Rechteck: eine Scheckkarte an den Bildschirm halten und die Größe ändern, bis beide genau gleich groß sind, dann **Übernehmen**.
