const { applyTargetNativeOnnxPolicy, electronBuilderTarget } = require('./onnx-target-policy.cjs');

module.exports = async function selectTargetNativeOnnx(context) {
  const target = electronBuilderTarget(context);
  applyTargetNativeOnnxPolicy(context.packager.config, target);
};
