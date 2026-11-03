# Entwickeln

Die Tabs **Belichtung**, **Farbe**, **Details** und **Effekte** enthalten die Werkzeuge für das ganze Bild. Jedes Werkzeug ist ein Abschnitt, der sich mit einem Klick auf seinen Kopf auf- und zuklappen lässt. Bearbeitungen sind nicht-destruktiv: Die RAW-Datei bleibt unverändert, jeder Schritt lässt sich mit {taste:undo} rückgängig machen.

Die KI-Werkzeuge dieser Tabs (Verbessern, Neu ausleuchten, Objektivunschärfe, Atmosphäre) stehen im Kapitel [KI-Werkzeuge](ki-werkzeuge).

## Bedienung der Werkzeuge

### Regler

- Doppelklick auf den Namen eines Reglers setzt ihn zurück.
- Nach einem Klick auf einen Regler ändern ihn die Pfeiltasten und das Mausrad, mit Umschalt grob, mit Alt fein. Ohne diesen Klick scrollt das Mausrad nur den Werkzeugbereich.
- Ein Zug am Regler ist ein Rückgängig-Schritt.
- Regler, die gerade nichts bewirken, bleiben sichtbar, sind aber ausgegraut.

### Knöpfe im Kopf eines Werkzeugs

- **↺** setzt das Werkzeug zurück. Mit gedrückter Umschalttaste und gehaltener Maustaste auf ↺ wird die Wirkung des Werkzeugs nur vorübergehend ausgeblendet: so lässt sich vergleichen, was es beiträgt.
- Ein durchgestrichenes **Auge** erscheint, wenn die Wirkung eines Werkzeugs ausgeblendet ist; ein Klick darauf blendet sie wieder ein.
- **A** stellt dieses eine Werkzeug automatisch ein, siehe [Automatisch anpassen](entwickeln#Automatisch anpassen).
- Das **Übertragen**-Symbol erscheint, wenn mehrere Bilder ausgewählt sind: Es überträgt nur dieses Werkzeug auf die anderen ausgewählten Bilder.
- **ⓘ** zeigt eine Kurzhilfe; ein Klick darauf öffnet dieses Handbuch an der passenden Stelle.

### Schnellbearbeitung

Eine der Tasten Q, A, D, F, V oder T gedrückt halten und am Mausrad drehen ändert Belichtung, Kontrast, Sättigung, Lichter, Tiefen bzw. Temperatur, ohne zum Regler zu wechseln. Ist eine Ebene gewählt, wirkt es auf sie.

### Auf einer Ebene

Ist eine Ebene gewählt, ändern viele Regler in Belichtung, Farbe und Details nur diese Ebene. Werkzeuge, die nur aufs ganze Bild wirken, sind dann ausgegraut. Mehr dazu unter [Ebenen & Masken](ebenen-masken).

## Automatisch anpassen

**Automatisch anpassen** ({taste:auto-adjust}, Knopf oben im Tab Belichtung oder Menü Entwickeln) stellt Weißabgleich, Belichtung, Lichter, Tiefen, Weiß, Schwarz, Kontrast und Dynamik in einem Schritt ein. Das Menü neben dem Knopf legt fest, was gesetzt wird; dort lassen sich auch die Tonwerte (Schwarz- und Weißpunkt) dazunehmen. Sind mehrere Bilder ausgewählt, werden alle angepasst.

Der Regler **Stärke** darunter blendet zwischen dem Zustand vorher (0 %) und dem vollen Ergebnis (100 %) über. Er ist erst nach einer automatischen Anpassung des Bildes aktiv.

Die Helligkeit richtet sich nach dem Bild, das die Kamera selbst entwickelt hat: Ein Waldbild bleibt dunkler als ein Strandbild. Gesichter und das Hauptmotiv zählen mehr als der Rest. Der Weißabgleich wird an fast neutralen Stellen wie Steinen, Wolken oder weißen Wänden gemessen; warmes Abendlicht bleibt warm.

### A je Werkzeug

Werkzeuge, die sich selbst einstellen können, tragen im Kopf ein **A**. Es stellt nur dieses Werkzeug ein, für alle ausgewählten Bilder:

- **Weißabgleich**: aus den fast neutralen Stellen.
- **Belichtung**: Belichtung und Kontrast.
- **Lichter & Tiefen**: Lichter, Tiefen, Weiß und Schwarz.
- **Farbe**: Dynamik.
- **Tonwerte**: Schwarz- und Weißpunkt.
- **Drehen & Spiegeln**, **Trapezkorrektur**, **Objektivkorrektur**: siehe [Objektiv & Geometrie](objektiv).
- **Schärfen** und **Rauschreduzierung**: nach dem gemessenen Rauschen und der Unschärfe des Objektivs.
- **Verbessern**: siehe [KI-Werkzeuge](ki-werkzeuge#Verbessern).

Jedes A gibt es auch als Befehl in der Befehlssuche (Strg+K), etwa „Belichtung automatisch“; dort lässt sich ihm eine eigene Taste geben. Auf einer Ebene ist A deaktiviert.

## Belichtung

Grundlegende Helligkeit des Bildes.

- **Belichtung**: in Blendenstufen (EV), wie eine längere oder kürzere Belichtung.
- **Kontrast**: spreizt oder staucht die Tonwerte um die Mitte.
- **Helligkeit**: hellt vor allem die Mitteltöne auf oder ab, ohne Lichter und Schatten so stark zu verschieben wie die Belichtung.

Alt beim Ziehen an **Belichtung** zeigt, wo Lichter ausfressen. Beim Überfahren eines Reglers markiert das Histogramm den Bereich, den er trifft.

## Lichter & Tiefen

Holt Zeichnung aus hellen und dunklen Bereichen, ohne den Rest zu verändern.

- **Lichter**: nach links holt ausgefressene Bereiche wie Himmel zurück.
- **Tiefen**: nach rechts hellt Schatten auf.
- **Weiß** und **Schwarz**: setzen den hellsten und dunkelsten Punkt.

Alt beim Ziehen zeigt ausgefressene bzw. abgesoffene Stellen. Feine Linien, Äste und Texturen behalten dabei ihren Kontrast.

## Tonwerte

Schwarz- und Weißpunkt mit Histogramm, für alle Kanäle zusammen (**RGB**) oder einzeln (**Rot**, **Grün**, **Blau**).

- **Eingang Schwarz** und **Eingang Weiß**: was zu Schwarz bzw. Weiß wird.
- **Mitteltöne**: über 1 heller, unter 1 dunkler.
- **Ausgang Schwarz** und **Ausgang Weiß**: begrenzen den Ausgabebereich, etwa für ein mattes Schwarz.

Beim Ziehen an den Eingangsgriffen zeigt das Bild, was abgeschnitten wird. Die Kanäle einzeln eignen sich, um einen Farbstich zu entfernen.

## Gradationskurve

Freie Tonkurve für gezielten Kontrast.

- Kanäle **RGB**, **Rot**, **Grün**, **Blau** und **Luma**. **Luma** ändert nur die Helligkeit: Kontrast ohne Farbverschiebung, Farbton und Sättigung bleiben.
- Klicken setzt einen Punkt, Ziehen verschiebt ihn, Rechtsklick entfernt ihn.
- Das Fadenkreuz rechts neben den Kanälen schaltet die [Zielkorrektur](entwickeln#Zielkorrektur) ein.

Die Kurve gibt es auch auf Ebenen.

## Zielkorrektur

Statt am Regler wird im Bild gezogen: Das Fadenkreuz neben den Kanälen der **Gradationskurve** oder neben **Farbton**, **Sättigung** und **Helligkeit** im **Farbeditor** schaltet sie ein.

1. Das Fadenkreuz anklicken.
2. Im Bild auf die Stelle zeigen, die sich ändern soll.
3. Nach oben oder unten ziehen.

In der Gradationskurve ändert sich die Kurve beim Ton dieser Stelle; im Farbeditor ändern sich die Farbbereiche, die in der Farbe der Stelle stecken. Esc beendet die Zielkorrektur. Sie wirkt auf die gewählte Ebene oder das ganze Bild.

## Basismerkmale

Die **Kurve**, auf der alle anderen Regler aufsetzen:

- **Kamera**: die Tonkurve des Bildes, das die Kamera selbst entwickelt hat, für jedes Bild angepasst. Helligkeit, Kontrast und Lichter sehen aus wie in der Kamera. Standard für neue Bilder.
- **Film Standard**: filmähnliche Kurve mit weicher Schulter in den Lichtern.
- **Film hoher Kontrast**: wie Film Standard, mit mehr Kontrast.
- **Linear**: ohne Kurve, Ausgangspunkt für eine eigene Gradation.

Steckt in der Datei kein passendes Kamerabild (etwa bei zusammengefügten DNGs), bleibt es bei Film Standard.

## Weißabgleich

Neutralisiert oder verändert die Farbe des Lichts.

- **Modus**: **Aufnahme** übernimmt den Wert der Kamera, **Benutzerdefiniert** gilt, sobald ein Regler bewegt wird.
- **Kelvin**: Farbtemperatur; höhere Werte machen das Bild wärmer.
- **Tönung**: zwischen Grün und Magenta.
- **Weißabgleich-Pipette** ({taste:wb-picker}): auf eine neutrale, graue Stelle im Bild klicken. Schon beim Überfahren zeigt das Bild das Ergebnis, eine Lupe neben dem Zeiger nennt Kelvin und Tönung.

Auf einer Ebene stehen **Temperatur** und **Tönung** zur Verfügung: Sie machen den Bereich der Ebene wärmer oder kühler als den Rest.

## Farbe

- **Sättigung**: verstärkt oder mindert alle Farben gleich.
- **Dynamik**: verstärkt vor allem schwache Farben und schont Hauttöne; meist die bessere Wahl.

## Farbeditor

Ändert einzelne Farben: acht Bereiche von Rot über Orange, Gelb, Grün, Cyan, Blau und Lila bis Magenta.

- **Farbton**, **Sättigung**, **Helligkeit**: jeweils ein Regler je Farbbereich, etwa um alle Blautöne dunkler zu machen.
- **Einzeln**: alle drei Eigenschaften für einen gewählten Farbbereich.
- **Eigene**: frei festgelegte Farbbereiche, siehe unten.

Das Fadenkreuz schaltet die [Zielkorrektur](entwickeln#Zielkorrektur) ein.

### Eigene Farbbereiche

Für eine Farbe, die zwischen die festen Bereiche fällt, etwa ein bestimmtes Türkis:

1. **Aufnehmen** wählen und im Bild auf die Farbe klicken.
2. Mit **Farbe**, **Breite** und **Weichheit** festlegen, welche Farbtöne erfasst werden.
3. Unter **Ändern** Farbton, Sättigung und Helligkeit dieser Farben ändern.

Bis zu acht eigene Bereiche sind möglich. Fast neutrale Farben bleiben unberührt. Eigene Bereiche gelten nur für das ganze Bild, nicht auf Ebenen.

## Hauttöne

Macht Haut gleichmäßiger und ändert ihren Farbton, ohne den Rest des Bildes zu treffen.

1. **Hautton aufnehmen** wählen und auf eine typische Hautstelle klicken.
2. **Bereich**: wie weit um diesen Ton herum Farben als Haut gelten.
3. Unter **Gleichmäßigkeit** gleichen **Farbton**, **Sättigung** und **Helligkeit** fleckige oder gerötete Haut an.
4. Unter **Hautton verändern** wird der Hautton als Ganzes verschoben.

## Farbbalance

Färbt Schatten, Mitteltöne und Lichter getrennt, etwa kühle Schatten und warme Lichter (Color Grading).

- **Dreiwege** zeigt Farbräder für **Schatten**, **Mitteltöne** und **Lichter** nebeneinander.
- **Master** färbt das ganze Bild.
- **Schatten**, **Mitteltöne** und **Lichter** zeigen ein Rad groß.

Die Mitte eines Rades ist neutral; je weiter nach außen, desto kräftiger die Färbung.

## Schwarzweiß

Wandelt das Bild in Schwarzweiß um.

- **Schwarzweiß aktivieren** schaltet die Umwandlung ein.
- **Farbempfindlichkeit**: wie hell jede ursprüngliche Farbe im Schwarzweißbild wird, wie Farbfilter vor dem Objektiv. Blau abgedunkelt ergibt etwa einen dramatischen Himmel.
- **Tonung**: färbt **Schatten** und **Lichter**, etwa für Sepia oder Split-Toning.

## Schärfen

Gleicht die leichte Unschärfe aus Objektiv und Sensor aus.

- **Stärke**: wie kräftig geschärft wird.
- **Radius**: wie breit die Kanten sind, die verstärkt werden.
- **Schwelle**: schont ruhige Flächen und Rauschen; höher heißt, nur deutliche Kanten werden geschärft.
- **Details**: wie geschärft wird. Bei 0 klassisches Unscharf maskieren, das auch leicht unscharfe Bilder knackiger macht, aber Säume erzeugen kann. Bei 100 wird die Unschärfe des Objektivs zurückgerechnet: feinere Details, kaum Säume.

Alt beim Ziehen zeigt in Grau, was der Regler bewirkt. Zum Beurteilen auf 100 % zoomen.

## Rauschreduzierung

Klassische Rauschreduzierung, auch zusätzlich zum KI-Entrauschen im Abschnitt [Verbessern](ki-werkzeuge#Verbessern).

- **Luminanz**: glättet das Helligkeitsrauschen.
- **Farbe**: entfernt bunte Flecken.

Alt beim Ziehen zeigt, was entfernt wird: Körnchen und Farbflecken sind richtig, erkennbare Umrisse heißen, dass es zu weit geht.

## Klarheit

- **Klarheit**: Kontrast in mittelgroßen Strukturen; gibt Landschaften und Architektur Tiefe. Negative Werte machen das Bild weicher.
- **Struktur**: Kontrast in feinen Details wie Stoff, Rinde oder Haut.

## Dunst entfernen

**Stärke** nach rechts entfernt Dunst und Nebel und gibt fernen Bereichen Kontrast und Farbe zurück. Nach links fügt Dunst hinzu. Auch auf Ebenen, etwa nur für den Himmel.

## Leuchten

Lässt Lichter leuchten und in ihre Umgebung strahlen, wie der Orton-Effekt.

- **Stärke**: wie hell das Leuchten ist.
- **Größe**: wie weit es reicht.

## Vignette

**Stärke** dunkelt die Bildränder ab (negativ) oder hellt sie auf (positiv), um den Blick in die Mitte zu lenken. Für die Randabschattung des Objektivs gibt es die [Objektivkorrektur](objektiv#Objektivkorrektur).

## Körnung

Fügt Filmkorn hinzu.

- **Stärke**: wie deutlich das Korn ist.
- **Größe**: wie grob es ist.

## Prozessversionen

Silberkorn merkt sich bei jeder Bearbeitung, mit welcher Version der Bildberechnung sie entstand. Ältere Bearbeitungen sehen deshalb auch nach Updates unverändert aus; neue und zurückgesetzte Bilder nutzen die aktuelle Version.

Neuere Versionen bewahren unter anderem den Farbton gesättigter Farben besser, verschieben mit mehr Kontrast keine Farbtöne mehr, rekonstruieren teilweise ausgebrannte Lichter und lassen den Radius beim Schärfen genauer wirken.

**Entwickeln → Auf aktuelle Prozessversion heben** stellt die ausgewählten Bilder auf die aktuelle Version um. Das Aussehen kann sich dabei leicht ändern.

> Hotpixel, einzelne zu helle Sensorpunkte, entfernt Silberkorn unabhängig von der Prozessversion immer.
