/*
 * Rain Classroom (Yuketang) student-side read-only adapter.
 *
 * This adapter deliberately exposes only assignment/exam metadata. It never
 * reads questions, answers, exam content, or submits anything.
 */
(function () {
  function ApiError(code, message) {
    this.name = "ApiError";
    this.code = code;
    this.message = message || code;
  }
  ApiError.prototype = Object.create(Error.prototype);

  function asInt(value) {
    if (typeof value === "number" && isFinite(value)) return Math.trunc(value);
    if (typeof value === "string" && /^-?\d+$/.test(value)) return parseInt(value, 10);
    return null;
  }

  function clean(value, max) {
    return String(value == null ? "" : value)
      .replace(/\s+/g, " ")
      .trim()
      .slice(0, max);
  }

  function headers(ctx, classroomId) {
    const h = {
      Accept: "application/json, text/plain, */*",
      "Accept-Language": "zh-CN,zh;q=0.9",
      Referer: ctx.config.baseUrl + "/v2/web/index",
      Origin: ctx.config.baseUrl,
      xtbz: "ykt",
      "X-Requested-With": "XMLHttpRequest",
    };
    const csrf = ctx.session.get("csrftoken");
    if (csrf) h["X-CSRFToken"] = csrf;
    if (classroomId !== null && classroomId !== undefined) {
      h["classroom-id"] = String(classroomId);
    }
    return h;
  }

  function requestJson(ctx, path, classroomId) {
    const response = ctx.http.request({
      url: ctx.config.baseUrl + path,
      method: "GET",
      headers: headers(ctx, classroomId),
    });
    if (response.error) {
      throw new ApiError("NETWORK", response.error);
    }
    if (response.status === 401 || response.status === 403) {
      throw new ApiError("AUTH_EXPIRED", "HTTP " + response.status);
    }
    if (response.status >= 300 && response.status < 400) {
      throw new ApiError("AUTH_EXPIRED", "session redirect");
    }
    if (response.status === 429) {
      throw new ApiError("RATE_LIMITED", "HTTP 429");
    }
    if (response.status < 200 || response.status >= 300) {
      throw new ApiError("HTTP_ERROR", "HTTP " + response.status);
    }
    try {
      return JSON.parse(response.body);
    } catch (error) {
      throw new ApiError("SCHEMA_CHANGED", "接口返回非 JSON");
    }
  }

  function failure(error, tasks, warnings) {
    const code = error && error.code ? error.code : "RUNTIME_ERROR";
    return {
      tasks: tasks || [],
      complete: false,
      error: code,
      warnings: (warnings || []).concat(error && error.message ? [String(error.message)] : []),
    };
  }

  function courseEntries(response) {
    const list = response && response.data && response.data.list;
    if (!Array.isArray(list)) {
      throw new ApiError("SCHEMA_CHANGED", "课程列表缺少 data.list");
    }
    const courses = [];
    for (let i = 0; i < list.length; i += 1) {
      const item = list[i] || {};
      const classroomId = asInt(item.classroom_id);
      const course = item.course || {};
      const courseId = asInt(course.id);
      if (classroomId === null || courseId === null) continue;
      courses.push({
        classroomId: classroomId,
        courseId: courseId,
        name: clean(course.name || item.name || "未命名课程", 120),
        term: asInt(item.term),
      });
    }
    return courses;
  }

  function currentCourses(courses) {
    let latest = null;
    for (let i = 0; i < courses.length; i += 1) {
      const term = courses[i].term;
      if (term !== null && term > 0 && (latest === null || term > latest)) latest = term;
    }
    if (latest === null) return courses;
    const filtered = [];
    for (let i = 0; i < courses.length; i += 1) {
      if (courses[i].term === latest) filtered.push(courses[i]);
    }
    return filtered.length > 0 ? filtered : courses;
  }

  function dedupeKey(course, title, startsAt, dueAt) {
    return [
      course.classroomId,
      clean(title, 300).toLowerCase(),
      startsAt === null ? 0 : startsAt,
      dueAt === null ? 0 : dueAt,
    ].join("|");
  }

  function leafTask(course, leaf, baseUrl) {
    if (!leaf || leaf.is_score !== true) return null;
    const dueAt = asInt(leaf.score_deadline);
    if (dueAt === null || dueAt <= 0) return null;
    const leafId = asInt(leaf.leafinfo_id) || asInt(leaf.id);
    if (leafId === null) return null;
    const title = clean(leaf.name, 300);
    if (!title) return null;
    const leafType = asInt(leaf.leaf_type);
    const startValue = asInt(leaf.start_time);
    const startsAt = startValue !== null && startValue > 0 ? startValue : null;
    const progress = leaf.progress || {};
    const done = asInt(progress.done) || 0;
    const total = asInt(progress.total) || 0;
    const status = clean(progress.status || "pending", 120);
    const completed =
      (total > 0 && done >= total) ||
      /完成|已交|已批阅/.test(status);
    let kind = "assignment";
    if (leafType === 6) kind = "exam";
    else if (leafType === 9) kind = "quiz";
    return {
      remoteId: "leaf:" + course.courseId + ":" + leafId,
      title: title,
      course: course.name,
      kind: kind,
      dueAt: dueAt,
      startsAt: startsAt,
      status: status,
      completed: completed,
      url: baseUrl + "/v2/web/studentLog/" + course.classroomId,
    };
  }

  function fetchTasks(ctx) {
    const baseUrl = ctx.config.baseUrl;
    const warnings = [];
    let tasks = [];
    const seen = {};
    let complete = true;

    let courses;
    try {
      courses = currentCourses(courseEntries(requestJson(ctx, "/v2/api/web/courses/list?identity=2", null)));
    } catch (error) {
      return failure(error, [], []);
    }

    for (let i = 0; i < courses.length; i += 1) {
      const course = courses[i];
      try {
        const progressPath =
          "/c27/online_courseware/schedule/course/progress/?classroom_id=" +
          course.classroomId +
          "&course_id=" +
          course.courseId;
        const response = requestJson(ctx, progressPath, course.classroomId);
        const sections = response && response.data && response.data.course_progress;
        if (!Array.isArray(sections)) {
          throw new ApiError("SCHEMA_CHANGED", "课程进度缺少 data.course_progress");
        }
        for (let s = 0; s < sections.length; s += 1) {
          const leaves = sections[s] && sections[s].section_leaf_list;
          if (!Array.isArray(leaves)) continue;
          for (let l = 0; l < leaves.length; l += 1) {
            const task = leafTask(course, leaves[l], baseUrl);
            if (task === null) continue;
            const key = dedupeKey(course, task.title, task.startsAt, task.dueAt);
            if (seen[key] === undefined) {
              seen[key] = tasks.length;
              tasks.push(task);
            }
          }
        }
      } catch (error) {
        if (error && (error.code === "AUTH_EXPIRED" || error.code === "RATE_LIMITED")) {
          return failure(error, tasks, warnings);
        }
        complete = false;
        warnings.push(course.name + " 拉取失败");
      }
      if (i + 1 < courses.length) ctx.sleep(1200);
    }

    try {
      const response = requestJson(ctx, "/api/v3/classroom/on-lesson-upcoming-exam", null);
      const exams = response && response.data && response.data.upcomingExam;
      if (!Array.isArray(exams)) {
        throw new ApiError("SCHEMA_CHANGED", "考试预告缺少 data.upcomingExam");
      }
      const courseByClassroom = {};
      for (let i = 0; i < courses.length; i += 1) {
        courseByClassroom[courses[i].classroomId] = courses[i];
      }
      for (let i = 0; i < exams.length; i += 1) {
        const exam = exams[i] || {};
        const classroomId = asInt(exam.classroom_id);
        const course = classroomId === null ? null : courseByClassroom[classroomId];
        const title = clean(exam.title, 300);
        if (!title) continue;
        const startSeconds = asInt(exam.start_time);
        const endSeconds = asInt(exam.end_time);
        const startsAt = startSeconds !== null && startSeconds > 0 ? startSeconds * 1000 : null;
        const dueAt = endSeconds !== null && endSeconds > 0 ? endSeconds * 1000 : startsAt;
        if (dueAt === null) continue;
        const shortClassroom = course ? course.classroomId : classroomId;
        const keyCourse = {
          classroomId: shortClassroom === null ? 0 : shortClassroom,
        };
        const key = dedupeKey(keyCourse, title, startsAt, dueAt);
        if (seen[key] !== undefined) {
          tasks[seen[key]].kind = "exam";
          continue;
        }
        const remoteId = asInt(exam.id);
        if (remoteId === null) continue;
        const url =
          classroomId === null
            ? baseUrl + "/v2/web/index"
            : baseUrl + "/v2/web/studentLog/" + classroomId;
        seen[key] = tasks.length;
        tasks.push({
          remoteId: "exam:" + remoteId,
          title: title,
          course: course ? course.name : clean(exam.classroom_name || "未命名课程", 120),
          kind: "exam",
          dueAt: dueAt,
          startsAt: startsAt,
          status: "pending",
          completed: false,
          url: url,
        });
      }
    } catch (error) {
      if (error && (error.code === "AUTH_EXPIRED" || error.code === "RATE_LIMITED")) {
        return failure(error, tasks, warnings);
      }
      complete = false;
      warnings.push("考试预告拉取失败");
    }

    tasks.sort(function (left, right) {
      const leftTarget = left.startsAt || left.dueAt || Number.MAX_SAFE_INTEGER;
      const rightTarget = right.startsAt || right.dueAt || Number.MAX_SAFE_INTEGER;
      return leftTarget - rightTarget;
    });
    ctx.log.info("yuketang adapter fetched " + tasks.length + " tasks");
    return { tasks: tasks, complete: complete, warnings: warnings };
  }

  globalThis.adapter = { fetchTasks: fetchTasks };
})();
