// SDK smoke-test adapter. It deliberately performs no network access.
// The desktop app loads this through the QuickJS adapter runtime.
globalThis.adapter = {
  fetchTasks(ctx) {
    ctx.log.info("mock adapter started");
    const now = Date.now();
    return {
      complete: true,
      tasks: [
        {
          remoteId: "sdk-smoke-1",
          title: "适配器 SDK 自检任务",
          course: "Mock 课程",
          kind: "assignment",
          dueAt: now + 60 * 60 * 1000,
          status: "pending",
          completed: false,
          url: "",
        },
      ],
    };
  },
};
