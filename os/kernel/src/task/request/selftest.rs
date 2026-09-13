//! 捕获预算的只读视图仅供隔离启动断言，不改变请求执行。

pub(crate) fn progress(request: &super::DrainRequest) -> (usize, usize) {
    (request.budget, request.work_done)
}
