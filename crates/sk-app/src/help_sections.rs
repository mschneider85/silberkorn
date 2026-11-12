//! Short help for the sections of the tool panel (the info symbol in their header) and where the manual
//! says more.

/// (section title as shown, short help, "kapitel#Überschrift" in the manual)
pub const SECTION_HELP: &[(&str, &str, &str)] = &[
    // Bibliothek
    ("Katalog", "Alle Bilder des Katalogs und die zuletzt importierten; im Menü daneben das Bereinigen fehlender Dateien.", "bibliothek#Katalog"),
    ("Ordner", "Die Ordner als Baum; ein Klick zeigt ihre Bilder, + öffnet einen anderen Ordner.", "bibliothek#Ordner"),
    ("Alben", "Sammlungen von Bildern unabhängig vom Ordner, auch als Filteralben, die sich selbst füllen.", "bibliothek#Alben"),
    (
        "Personen",
        "Die auf den Bildern erkannten Personen: ein Klick zeigt nur ihre Bilder, Doppelklick benennt, Ziehen auf eine andere führt zusammen.",
        "bibliothek#Personen",
    ),
    // Title shown as "Session · Name".
    ("Session", "Die geöffnete Session: ihre Ordner, Importieren und der Weg zurück zum Katalog.", "bibliothek#Sessions"),
    ("Session-Favoriten", "Häufig gebrauchte Ordner der Session für den schnellen Zugriff.", "bibliothek#Sessions"),
    // Objektiv & Geometrie
    (
        "Objektivkorrektur",
        "Korrigiert Verzeichnung, Randabschattung, chromatische Aberration und Farbsäume, aus den Kameradaten oder einem Objektivprofil. Entzerrt auch Fisheye-Bilder.",
        "objektiv#Objektivkorrektur",
    ),
    ("Farbfehler", "Chromatische Aberration und lila oder grüne Farbsäume an Kanten, im eingeklappten Bereich der Objektivkorrektur.", "objektiv#Chromatische Aberration"),
    ("Fisheye", "Rechnet ein Fisheye-Bild in ein normales Weitwinkelbild um, im eingeklappten Bereich der Objektivkorrektur.", "objektiv#Fisheye"),
    ("Zuschnitt", "Legt den Bildausschnitt fest, frei oder in einem Seitenverhältnis, im Hoch- oder Querformat.", "objektiv#Zuschnitt"),
    ("Drehen & Spiegeln", "Richtet einen schiefen Horizont gerade, dreht um 90° oder spiegelt das Bild.", "objektiv#Drehen & Spiegeln"),
    (
        "Trapezkorrektur",
        "Richtet stürzende Linien auf, etwa bei Gebäuden: mit Reglern, mit ins Bild gezogenen Linien oder automatisch.",
        "objektiv#Trapezkorrektur",
    ),
    // Belichtung
    ("Belichtung", "Grundhelligkeit und Kontrast des Bildes. Belichtung in Blendenstufen, Helligkeit vor allem für die Mitteltöne.", "entwickeln#Belichtung"),
    (
        "Lichter & Tiefen",
        "Holt Zeichnung aus hellen und dunklen Bereichen zurück, etwa einen ausgefressenen Himmel oder dunkle Schatten. Weiß und Schwarz setzen die Endpunkte.",
        "entwickeln#Lichter & Tiefen",
    ),
    (
        "Neu ausleuchten",
        "Hellt Vorder- und Hintergrund getrennt auf oder ab und färbt sie wärmer oder kühler, etwa bei Gegenlicht. Die KI schätzt dafür die Entfernungen im Bild.",
        "ki-werkzeuge#Neu ausleuchten",
    ),
    ("Tonwerte", "Schwarz- und Weißpunkt und Mitteltöne mit Histogramm, für alle Kanäle zusammen oder einzeln, etwa gegen einen Farbstich.", "entwickeln#Tonwerte"),
    (
        "Gradationskurve",
        "Freie Tonkurve für gezielten Kontrast, für RGB, die einzelnen Farbkanäle oder nur die Helligkeit (Luma).",
        "entwickeln#Gradationskurve",
    ),
    // Farbe
    ("Look angleichen", "Überträgt den Look von Vorlagen auf die Auswahl, mit den normalen Reglern.", "vergleichen#Look angleichen"),
    ("Basismerkmale", "Die Grundkurve, auf der alle Regler aufsetzen; Standard ist die Tonkurve der Kamera.", "entwickeln#Basismerkmale"),
    ("Weißabgleich", "Neutralisiert die Farbe des Lichts oder macht das Bild wärmer oder kühler, mit Reglern oder per Pipette auf einer grauen Stelle.", "entwickeln#Weißabgleich"),
    ("Farbe", "Sättigung verstärkt alle Farben gleich, Dynamik vor allem schwache Farben und schont Hauttöne.", "entwickeln#Farbe"),
    (
        "Farbeditor",
        "Ändert Farbton, Sättigung und Helligkeit einzelner Farben, etwa nur das Blau des Himmels. Eigene Farbbereiche lassen sich im Bild aufnehmen.",
        "entwickeln#Farbeditor",
    ),
    ("Hauttöne", "Macht Haut gleichmäßiger und verschiebt den Hautton, ohne den Rest des Bildes zu verändern.", "entwickeln#Hauttöne"),
    ("Farbbalance", "Färbt Schatten, Mitteltöne und Lichter getrennt mit Farbrädern, etwa kühle Schatten und warme Lichter.", "entwickeln#Farbbalance"),
    ("Schwarzweiß", "Wandelt in Schwarzweiß um; legt fest, wie hell jede Farbe wird, und tont Schatten und Lichter.", "entwickeln#Schwarzweiß"),
    // Details
    (
        "Verbessern",
        "Raw-Details holt feinere Details mit weniger Farbsäumen und Moiré aus den RAW-Daten, KI-Entrauschen entfernt Rauschen. Einmal je Bild im Hintergrund berechnet.",
        "ki-werkzeuge#Verbessern",
    ),
    ("Rauschreduzierung", "Klassische Rauschreduzierung: Luminanz glättet das Helligkeitsrauschen, Farbe die bunten Flecken.", "entwickeln#Rauschreduzierung"),
    ("Schärfen", "Gleicht die leichte Unschärfe aus Objektiv und Sensor aus. Am besten bei 100 % beurteilen.", "entwickeln#Schärfen"),
    ("Klarheit", "Klarheit verstärkt den Kontrast mittelgroßer Strukturen, Struktur den feiner Details. Negative Werte machen weicher.", "entwickeln#Klarheit"),
    ("Dunst entfernen", "Entfernt Dunst und Nebel und gibt fernen Bereichen Kontrast und Farbe zurück; negative Werte fügen Dunst hinzu.", "entwickeln#Dunst entfernen"),
    // Effekte
    (
        "Objektivunschärfe",
        "Zeichnet Vorder- und Hintergrund weich wie eine offene Blende, mit frei wählbarem Fokus. Die KI schätzt dafür die Entfernungen im Bild.",
        "ki-werkzeuge#Objektivunschärfe",
    ),
    ("Atmosphäre", "Legt Dunst über das Bild, der mit der Entfernung dichter wird. Die KI schätzt dafür die Entfernungen im Bild.", "ki-werkzeuge#Atmosphäre"),
    ("Leuchten", "Lässt Lichter leuchten und in ihre Umgebung strahlen, wie der Orton-Effekt.", "entwickeln#Leuchten"),
    ("Vignette", "Dunkelt die Bildränder ab oder hellt sie auf, um den Blick zur Mitte zu lenken.", "entwickeln#Vignette"),
    ("Körnung", "Fügt Filmkorn hinzu; Größe bestimmt, wie grob es ist.", "entwickeln#Körnung"),
    // Ebenen
    ("Ebenen", "Ebenen ändern nur einen Teil des Bildes. Jede hat eine Maske, die festlegt wo, und Anpassungen, die festlegen was.", "ebenen-masken#Ebenen"),
    (
        "Maske",
        "Wo die gewählte Ebene wirkt: malen, Verläufe ziehen, Motiv, Himmel oder Objekt auswählen und auf Helligkeit oder Farben eingrenzen.",
        "ebenen-masken#Maske",
    ),
    ("Anpassungen", "Was die gewählte Ebene ändert. Die Regler in Belichtung, Farbe und Details wirken auf sie.", "ebenen-masken#Anpassungen"),
    // Retusche
    (
        "Werkzeuge",
        "Reparieren und Klonen beseitigen Flecken und kleine Störungen, Entfernen füllt größere Objekte per KI. Staub und Flecken finden sucht Sensorstaub.",
        "retusche#Werkzeuge",
    ),
    ("Retuschen", "Alle Retuschen des Bildes: wählen, ändern, eine neue Quelle suchen oder löschen.", "retusche#Retuschen"),
    // Metadaten
    ("Bewertung", "Sterne und Farb-Label des Bildes oder der Auswahl; gestrichelt, was nur einige der ausgewählten Bilder haben.", "bibliothek#Bewertung"),
    ("Keywords", "Schlagwörter zum Wiederfinden der Bilder.", "bibliothek#Keywords"),
    ("Aufnahme", "Blende, Belichtungszeit, ISO und Brennweite, dazu Kamera und Objektiv.", "bibliothek#Aufnahme"),
    ("Datei", "Auflösung, Dateigröße und Ordner des Bildes.", "bibliothek#Aufnahme"),
    // Stile
    ("Filmsimulationen", "Fertige Looks nach klassischen Filmen; der Zeiger auf einem Stil zeigt ihn als Vorschau im Bild.", "vergleichen#Filmsimulationen"),
    ("Eigene Stile", "Selbst gespeicherte Stile, auf die Auswahl anzuwenden.", "vergleichen#Eigene Stile"),
    ("Neuer Stil", "Speichert Einstellungen des Bildes als Stil, um sie auf andere Bilder zu übertragen.", "vergleichen#Neuer Stil"),
    // Verlauf
    ("Schnappschüsse", "Gesicherte Stände der Bearbeitung, zu denen sich jederzeit zurückkehren lässt.", "verlauf#Schnappschüsse"),
    ("Verlauf", "Alle Bearbeitungsschritte; ein Klick springt zu einem früheren Stand.", "verlauf#Verlauf"),
    // Ausgabe
    ("Ausgaberezepte", "Vorlagen für den Export: Format, Größe, Farbraum und Ordner. Mehrere Rezepte lassen sich gleichzeitig nutzen.", "ausgabe#Ausgaberezepte"),
    ("Rezept bearbeiten", "Name, Format, Farbraum, Größe und Ziel des gewählten Ausgaberezepts.", "ausgabe#Rezept bearbeiten"),
    ("Wasserzeichen", "Fügt beim Export einen Text oder ein Logo ein.", "ausgabe#Wasserzeichen"),
    ("Metadaten", "Welche Metadaten der Export mitnimmt: Aufnahmedaten, Fotograf und Copyright.", "ausgabe#Metadaten"),
    ("Exportieren", "Exportiert die Auswahl mit allen eingeschalteten Rezepten.", "ausgabe#Exportieren"),
    ("Softproof", "Zeigt, wie das Bild in einem anderen Farbraum oder auf einem Drucker aussieht, und markiert Farben außerhalb.", "ausgabe#Softproof"),
    ("Druckvorschau", "Zeigt das Bild in Druckgröße auf dem gewählten Papierformat.", "ausgabe#Druckvorschau"),
    ("Druckseite", "Rand, Rahmen, Schnittmarken und Beschriftung: was außer dem Bild auf dem Blatt steht.", "ausgabe#Druckseite"),
    ("Drucken", "Druckt die ausgewählten Bilder oder speichert die Seiten als PDF oder JPEG; mit Druckanpassung, Schärfung und Druckerprofil.", "ausgabe#Drucken"),
    // Seitenleiste rechts neben dem Bild (dazu "Schnappschüsse" und "Verlauf" wie oben)
    ("Bild", "Name, Kamera, Belichtung und Größe des Bildes, dazu Bewertung und Farb-Label.", "ansicht#Seitenleiste"),
    ("Bearbeitung", "Was an diesem Bild geändert ist; ein Klick öffnet das Werkzeug.", "ansicht#Seitenleiste"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_unique() {
        for (i, (title, help, target)) in SECTION_HELP.iter().enumerate() {
            assert!(help.chars().count() <= 250, "{title}: zu lang");
            assert!(!target.is_empty(), "{title}: kein Ziel");
            assert!(SECTION_HELP[..i].iter().all(|(t, _, _)| t != title), "{title} doppelt");
        }
    }
}
