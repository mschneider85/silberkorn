# Vergleichen und Looks

Bilder einer Serie sollen oft gleich aussehen. Dafür gibt es die Vergleichsansicht mit Referenzbild, das Angleichen an Vorlagen und Stile.

## Auswahl nebeneinander vergleichen

Sind mehrere Bilder ausgewählt (Strg+Klick oder Umschalt+Klick), zeigt das Einzelbild sie nebeneinander, bis zu zwölf. Mausrad und Ziehen zoomen bzw. verschieben alle Bilder gemeinsam, ein Doppelklick wechselt zwischen Einpassen und Vergrößerung. So vergleicht man Schärfe oder Mimik an derselben Stelle.

Ein Klick macht ein Bild zum bearbeiteten (blau umrandet), ohne die Auswahl zu ändern; die Werkzeuge wirken dann auf dieses Bild. Abschalten lässt sich die Ansicht mit **Auswahl nebeneinander vergleichen** im Menü **Ansicht**.

### Referenzbild

**Referenzbild festlegen / entfernen** ({taste:reference}, Menü **Entwickeln** oder Rechtsklick auf ein Bild der Vergleichsansicht) heftet das aktuelle Bild an. Es bleibt links neben dem bearbeiteten Bild stehen, auch beim Durchblättern, und trägt die Beschriftung „Referenz“. So lassen sich alle Bilder einer Serie an einem Vorbild ausrichten.

## Look angleichen

**Look angleichen** (oben im Tab **Farbe** oder Menü **Entwickeln**) überträgt den Look einer oder mehrerer Vorlagen auf die ausgewählten Bilder: Helligkeit, Weißabgleich, Kontrast und Farbstimmung. Die Motive müssen nicht übereinstimmen. Das Ergebnis sind ganz normale Regler, die sich danach weiter bearbeiten lassen.

### Vorlagen

**Vorlage hinzufügen** bietet drei Quellen:

- **Bilddatei …**: ein JPEG, PNG, TIFF oder WebP, etwa ein Filmstill. Dateien lassen sich auch auf den Abschnitt ziehen.
- **Dieses Bild**: das geöffnete Bild, so wie es bearbeitet ist.
- **Referenzbild**: das angeheftete Bild der Vergleichsansicht.

Dieselben Befehle stehen im Menü **Entwickeln** als **Vorlage aus Bilddatei …**, **Dieses Bild als Vorlage** und **Referenzbild als Vorlage**.

Jede Vorlage erscheint als Kachel mit ihrer Ähnlichkeit zum aktuellen Bild („sehr ähnlich“, „ähnlich“, „anderes Motiv“). Ähnliche Vorlagen bestimmen den Look stärker. Bei einem anderen Motiv werden Belichtung, Tonwerte und Farben nur teilweise übernommen, damit etwa ein Foto mit viel hellem Himmel nicht jedes Bild überstrahlt. Das ✕ auf einer Kachel oder **Vorlage entfernen** im Rechtsklick-Menü nimmt sie heraus; eine Vorlage aus der Bibliothek öffnet ein Klick.

### Was übernommen wird

Unter **Übernehmen** wählen Schalter, was angeglichen wird:

- **Belichtung**: die mittlere Helligkeit.
- **Weißabgleich**: der Farbstich der fast neutralen Töne.
- **Tonwerte**: Schwarz- und Weißpunkt, Helligkeit und Kontrast.
- **Farben**: Sättigung und Farbeditor.
- **Farbstimmung**: die Farbbalance in Schatten, Mitten und Lichtern.

Eine Schwarzweiß-Vorlage, auch eine getönte, macht das Bild schwarzweiß und übernimmt die Tönung.

### Stärke

Nach dem Angleichen schwächt **Stärke** das Ergebnis stufenlos ab (0 bis 100 %). Darunter steht, was sich geändert hat und wie viel jede Vorlage beigetragen hat.

## Stile

Ein Stil ist ein gespeicherter Satz von Einstellungen, der sich mit einem Klick auf andere Bilder legen lässt. Der Tab **Stile** zeigt jeden Stil als kleine Vorschau des aktuellen Bildes. Die beiden Symbole im Kopf der Filmsimulationen wechseln zwischen **Vorschauen** und **Liste** (kleinere Bilder mit Beschreibung).

- **Wirkt auf** oben nennt, wohin ein Klick geht: die ausgewählten Bilder oder die gewählte Ebene.
- Zeiger auf einen Stil: Das Bild zeigt ihn groß, ohne ihn anzuwenden.
- Klick: Der Stil wird angewendet, auf mehrere Bilder als ein Rückgängig-Schritt.
- Ein Haken und ein blauer Rand markieren Stile, die schon wirken: Erneutes Anwenden würde nichts ändern.

Ein Stil setzt nur seine eigenen Werkzeuge; alle anderen bleiben, wie sie sind. So lassen sich Stile übereinander anwenden, etwa erst ein Farblook und danach eine Körnung.

### Filmsimulationen

Die **Filmsimulationen** sind mitgelieferte Looks nach bekannten Filmen wie Kodak Portra 400 oder Fuji Velvia 50. Sie sind nachempfunden, keine offiziellen Simulationen. Belichtung, Weißabgleich und Korrekturen des Bildes bleiben erhalten.

### Eigene Stile

Unter **Eigene Stile** stehen die selbst gespeicherten, mit der Zahl ihrer Werkzeuge; der Tooltip nennt sie. Der Papierkorb erscheint, wenn der Zeiger auf dem Stil steht (auch im Kontextmenü): **Stil löschen …**. Bilder, auf die er angewendet wurde, bleiben unverändert.

### Neuer Stil

1. Ein Bild so bearbeiten, wie der Stil aussehen soll.
2. Die gestrichelte Kachel **Als Stil sichern** am Ende der eigenen Stile anklicken.
3. Einen Namen eingeben.
4. Unter **Enthält** stehen die bearbeiteten Werkzeuge, alle gewählt. Ein Klick nimmt eines heraus oder wieder auf.
5. **Speichern**.

### Stile auf einer Ebene

Ist im Tab **Ebenen** eine Ebene gewählt, geht ein Stil oder eine Filmsimulation auf diese Ebene statt auf das ganze Bild. Die Deckkraft der Ebene ist dann seine Stärke, ihre Maske grenzt ihn ein. Was eine Ebene nicht kann (etwa Körnung oder Tonwerte), nennt die Statusleiste. Mehr dazu im Kapitel [Ebenen und Masken](ebenen-masken).
