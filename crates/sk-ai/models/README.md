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
