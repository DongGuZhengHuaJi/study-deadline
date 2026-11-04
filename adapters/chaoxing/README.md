# 学习通适配器

只读学生端适配器，返回本人账号的作业/考试元数据与截止时间。

- 登录入口：`https://passport2.chaoxing.com/login?fid=&refer=https%3A%2F%2Fi.chaoxing.com`
- 会话：仅提取 `*.chaoxing.com` Cookie，要求至少存在 `_uid` 或 `UID`
- 数据源：
  - `i.chaoxing.com/base`：验活并解析学校 `fid`
  - `mooc1-api.chaoxing.com/mycourse/backclazzdata`：JSON 课程列表
  - `mooc1-api.chaoxing.com/work/stu-work`：待交作业聚合
  - 作业 `data` 详情页：绝对截止时间（有请求上限）
  - `exam-ans/exam/test/examcode/examlist`：全校考试表（绝对起止时间）
  - `exam-ans/exam/phone/examcode`：考试列表为空时的相对时间回退
- 不读取题目/答案/试卷，不提交作业，不进入考试房间。

平台接口和置信度说明见 [`docs/platform/chaoxing.md`](../../docs/platform/chaoxing.md)。

## 测试

```bash
pnpm test:adapters
```

契约测试使用脱敏 fixtures（`tests/fixtures/`），不访问真实账号。
