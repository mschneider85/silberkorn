"""Rewrites MatMul(A[N,N], Unsqueeze(X[...,H], -1)) as Unsqueeze(MatMul(X, Transpose(A)), -1):
the same numbers, but a 4-D x 2-D product that DirectML accepts.

    python lama_dml.py lama_fp32.onnx lama_dml.onnx
"""
import sys, onnx
from onnx import helper, numpy_helper, shape_inference
import numpy as np

src, dst = sys.argv[1], sys.argv[2]
m = onnx.load(src)
g = m.graph
inferred = shape_inference.infer_shapes(m)
rank = {v.name: len(v.type.tensor_type.shape.dim) for v in list(inferred.graph.value_info) + list(g.input)}
producer = {o: n for n in g.node for o in n.output}
consts = {i.name: numpy_helper.to_array(i) for i in g.initializer}
def const_value(name):
    if name in consts:
        return consts[name]
    p = producer.get(name)
    if p is not None and p.op_type == "Constant":
        return numpy_helper.to_array(p.attribute[0].t)
    return None

minus1 = "lama_fix_axes_minus1"
g.initializer.append(numpy_helper.from_array(np.array([-1], dtype=np.int64), minus1))
new_nodes, done = [], 0
for n in g.node:
    if n.op_type == "MatMul" and rank.get(n.input[0]) == 2 and rank.get(n.input[1]) == 5:
        u = producer.get(n.input[1])
        axes = const_value(u.input[1]) if u is not None and u.op_type == "Unsqueeze" and len(u.input) > 1 else None
        if axes is not None and [int(a) for a in axes.ravel()] in ([-1], [4]):
            a, x, out = n.input[0], u.input[0], n.output[0]
            at, mm = n.name + "/fix_At", n.name + "/fix_mm"
            new_nodes += [
                helper.make_node("Transpose", [a], [at], name=n.name + "/fix_T", perm=[1, 0]),
                helper.make_node("MatMul", [x, at], [mm], name=n.name + "/fix_MatMul"),
                helper.make_node("Unsqueeze", [mm, minus1], [out], name=n.name + "/fix_Unsqueeze"),
            ]
            done += 1
            continue
    new_nodes.append(n)
del g.node[:]
g.node.extend(new_nodes)
print("umgebaut:", done)
onnx.checker.check_model(m)
onnx.save(m, dst)
