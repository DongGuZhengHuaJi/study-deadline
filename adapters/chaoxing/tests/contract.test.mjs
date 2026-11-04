import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const adapterSource = readFileSync(new URL("../index.js", import.meta.url), "utf8");
const fixture = (name) =>
  readFileSync(new URL(`./fixtures/${name}`, import.meta.url), "utf8");

const BASE = "https://i.chaoxing.com/base";
const COURSES =
  "https://mooc1-api.chaoxing.com/mycourse/backclazzdata?view=json&rss=1";
const WORK = "https://mooc1-api.chaoxing.com/work/stu-work";
const WORK_DETAIL = "https://mooc1.chaoxing.com/mooc-ans/mooc2/work/task";
const EXAMS = "https://mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist";
const EXAM_FALLBACK = "https://mooc1-api.chaoxing.com/exam-ans/exam/phone/examcode";

function loadAdapter() {
  const context = vm.createContext({ console });
  vm.runInContext(adapterSource, context, { filename: "chaoxing/index.js" });
  assert.equal(typeof context.adapter.fetchTasks, "function");
  return context.adapter;
}

function response(body) {
  return { status: 200, body };
}

function makeContext(routes) {
  return {
    config: {
      adapterId: "chaoxing",
      region: "default",
      baseUrl: "https://i.chaoxing.com",
      timezone: "Asia/Shanghai",
      locale: "zh-CN",
    },
    session: {
      get: (key) => (key === "fid" ? "1234" : null),
    },
    log: { debug() {}, info() {}, warn() {}, error() {} },
    sleep() {},
    http: {
      request(request) {
        for (const route of routes) {
          if (request.url.startsWith(route.prefix)) {
            return route.handler ? route.handler(request) : route.response;
          }
        }
        return { status: 404, body: "not found" };
      },
    },
  };
}

function standardRoutes(overrides = {}) {
  return [
    { prefix: BASE, response: response(fixture("base.html")), ...overrides.base },
    {
      prefix: COURSES,
      response: response(fixture("courses.json")),
      ...overrides.courses,
    },
    {
      prefix: WORK_DETAIL,
      response: response(fixture("work-detail-9001.html")),
      ...overrides.detail,
    },
    {
      prefix: WORK,
      response: response(fixture("stu-work.html")),
      ...overrides.work,
    },
    {
      prefix: EXAMS,
      response: response(fixture("examlist.html")),
      ...overrides.exams,
    },
    {
      prefix: EXAM_FALLBACK,
      response: response(fixture("exam-phone.html")),
      ...overrides.examFallback,
    },
  ];
}

test("parses aggregate work, detail deadline, and exam table", () => {
  const result = loadAdapter().fetchTasks(makeContext(standardRoutes()));

  assert.equal(result.error, undefined);
  assert.equal(result.complete, true);
  assert.equal(result.warnings.length, 0);

  const work = result.tasks.find((task) => task.title === "第三次作业");
  assert.ok(work, "pending work is returned");
  assert.equal(work.course, "概率论");
  assert.equal(work.completed, false);
  assert.equal(work.startsAt, Date.UTC(2026, 5, 21, 10, 51));
  assert.equal(work.dueAt, Date.UTC(2026, 5, 21, 12, 0));
  assert.match(work.url, /stucoursemiddle\?courseid=101/);
  assert.doesNotMatch(work.url, /enc=/);

  const completedWork = result.tasks.find(
    (task) => task.title === "已完成作业",
  );
  assert.ok(completedWork);
  assert.equal(completedWork.completed, true);

  const exam = result.tasks.find(
    (task) => task.title === "信号与系统期末考试",
  );
  assert.ok(exam);
  assert.equal(exam.kind, "exam");
  assert.equal(exam.startsAt, Date.UTC(2026, 6, 1, 1, 0));
  assert.equal(exam.dueAt, Date.UTC(2026, 6, 1, 3, 0));
  assert.equal(exam.completed, false);

  const completedExam = result.tasks.find(
    (task) => task.title === "已结束的考试",
  );
  assert.ok(completedExam);
  assert.equal(completedExam.completed, true);
});

test("falls back to the phone exam aggregate with relative time", () => {
  const before = Date.now();
  const result = loadAdapter().fetchTasks(
    makeContext(
      standardRoutes({
        exams: { response: response(fixture("examlist-empty.html")) },
      }),
    ),
  );
  const after = Date.now();

  assert.equal(result.error, undefined);
  assert.equal(result.complete, true);
  const exam = result.tasks.find((task) => task.title === "期中考试");
  assert.ok(exam);
  assert.equal(exam.startsAt, null);
  const expected = 2 * 86400000 + 3 * 3600000;
  assert.ok(exam.dueAt >= before + expected);
  assert.ok(exam.dueAt <= after + expected);
});

test("reports auth expiry when the base page is a login page", () => {
  const result = loadAdapter().fetchTasks(
    makeContext([
      { prefix: BASE, response: response(fixture("login.html")) },
    ]),
  );
  assert.equal(result.error, "AUTH_EXPIRED");
  assert.equal(result.complete, false);
});

test("reports auth expiry after a redirect to passport login", () => {
  const result = loadAdapter().fetchTasks(
    makeContext([
      {
        prefix: BASE,
        response: {
          status: 302,
          body: "",
          location:
            "https://passport2.chaoxing.com/login?fid=&refer=https%3A%2F%2Fi.chaoxing.com",
        },
      },
      {
        prefix: "https://passport2.chaoxing.com/login",
        response: response(fixture("login.html")),
      },
    ]),
  );
  assert.equal(result.error, "AUTH_EXPIRED");
});

test("keeps exams when the work aggregate fails", () => {
  const result = loadAdapter().fetchTasks(
    makeContext(
      standardRoutes({
        work: { response: { status: 500, body: "server error" } },
      }),
    ),
  );

  assert.equal(result.error, undefined);
  assert.equal(result.complete, false);
  assert.ok(
    result.tasks.some((task) => task.title === "信号与系统期末考试"),
    "exam table still parsed",
  );
  assert.ok(
    result.warnings.some((warning) => warning.includes("作业列表")),
    "work failure is reported as a warning",
  );
});
