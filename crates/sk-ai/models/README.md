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
