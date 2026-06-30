use llvmkit_ir::FunctionPipelineStep;

fn main() {
    let _ = FunctionPipelineStep::Pass("dce");
}
