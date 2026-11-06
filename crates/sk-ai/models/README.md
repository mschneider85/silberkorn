# Mitgelieferte Modelle

`demosaicnet_bayer.onnx`: DemosaicNet für Bayer-Sensoren aus
[mgharbi/demosaicnet](https://github.com/mgharbi/demosaicnet) (Commit `959e9d1`, `demosaicnet/data/bayer.pth`,
MIT-Lizenz, siehe `demosaicnet_LICENSE`), unverändert nach ONNX exportiert (Opset 13, Eingabe
1×3×768×768, Ausgabe 1×3×706×706):

```python
net = BayerDemosaick(pretrained=False).eval()
net.load_state_dict(torch.load("bayer.pth", weights_only=True, map_location="cpu"))
torch.onnx.export(net, torch.zeros(1, 3, 768, 768), "demosaicnet_bayer.onnx",
                  input_names=["mosaic"], output_names=["rgb"], opset_version=13, dynamo=False)
```

`span_x2.onnx`: SPAN 2× (Cheng Wan, Hongyuan Yu et al., „Swift Parameter-free Attention Network
for Efficient Super-Resolution“, NTIRE 2024) aus [hongyuanyu/SPAN](https://github.com/hongyuanyu/SPAN)
(Commit `c77a591`), die offiziellen Gewichte `spanx2_ch48.pth` (Apache-2.0, siehe `span_LICENSE`;
SHA-256 `0b20caee…d3f93`, wie bei OpenModelDB `2x-spanx2-ch48`), mit spandrel 0.4.2 geladen
(dabei werden die Faltungsblöcke zu einfachen Faltungen zusammengefasst) und unverändert nach ONNX
exportiert (Opset 17, Eingabe 1×3×512×512, Ausgabe 1×3×1024×1024):

```python
d = spandrel.ModelLoader().load_from_file("2x-spanx2-ch48.pth")
torch.onnx.export(d.model.eval(), torch.rand(1, 3, 512, 512), "span_x2.onnx",
                  input_names=["input"], output_names=["output"], opset_version=17, dynamo=False)
```

`keywords.bin`: die Textvektoren der Wörter in `keywords.txt` für die Keyword-Vorschläge, berechnet mit
dem Text-Encoder von SigLIP 2 (Google, Apache-2.0; siehe `models::CONTENT_TEXT`), als f16. Nach einer
Änderung der Liste neu erzeugen:

```
cargo run --release -p sk-ai --example keyword_vectors
```

# Selbst bereitgestellte Modelle

`lama_dml.onnx` (Release `lama-dml-1`): LaMa (Roman Suvorov et al., Samsung AI Center,
Apache-2.0) in Carves ONNX-Export ([Carve/LaMa-ONNX](https://huggingface.co/Carve/LaMa-ONNX),
Revision `c3c0c9e`, `lama_fp32.onnx`), mit `lama_dml.py` umgebaut: Die Fourier-Faltungen
multiplizieren dort eine 2-D-Matrix mit einem 5-D-Tensor, was DirectML ablehnt; das Skript
schreibt sie als gleichwertige Produkte eines 4-D-Tensors mit der transponierten Matrix (144
Knoten, größte Abweichung auf der CPU 0.0001 bei Werten bis 255):

```
python lama_dml.py lama_fp32.onnx lama_dml.onnx
```
