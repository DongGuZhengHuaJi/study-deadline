/*
 * Chaoxing / 学习通 student-side read-only adapter.
 *
 * Scope: the signed-in user's own assignment/exam METADATA only. This adapter
 * never reads questions, answers, paper content, submissions, or exam rooms.
 * It relies on the non-public student web endpoints documented in
 * docs/platform/chaoxing.md; those endpoints can change without notice.
 */
(function () {
  var BEIJING_OFFSET_MS = 8 * 3600 * 1000;
  var MAX_DETAIL_REQUESTS = 30;
  var DETAIL_GAP_MS = 250;
  var SOURCE_GAP_MS = 300;
  var MAX_WARNINGS = 20;

  var BASE_PAGE = "https://i.chaoxing.com/base";
  var COURSE_LIST =
    "https://mooc1-api.chaoxing.com/mycourse/backclazzdata?view=json&rss=1";
  var WORK_AGGREGATE = "https://mooc1-api.chaoxing.com/work/stu-work";
  var EXAM_LIST =
    "https://mooc1.chaoxing.com/exam-ans/exam/test/examcode/examlist?edition=1&nohead=0&fid=";
  var EXAM_AGGREGATE =
    "https://mooc1-api.chaoxing.com/exam-ans/exam/phone/examcode";

  function ApiError(code, message) {
    this.name = "ApiError";
    this.code = code;
    this.message = message || code;
  }
  ApiError.prototype = Object.create(Error.prototype);

  function asId(value) {
    if (typeof value === "number" && isFinite(value)) return String(Math.trunc(value));
    if (typeof value === "string" && /^\d+$/.test(value)) return value;
    return null;
  }

  function clean(value, max) {
    return String(value == null ? "" : value)
      .replace(/[\s ]+/g, " ")
      .trim()
      .slice(0, max);
  }

  function decodeHtml(value) {
    var text = String(value == null ? "" : value);
    text = text.replace(/&#x([0-9a-f]+);/gi, function (_, hex) {
      return String.fromCharCode(parseInt(hex, 16));
    });
    text = text.replace(/&#(\d+);/g, function (_, dec) {
      return String.fromCharCode(parseInt(dec, 10));
    });
    return text
      .replace(/&nbsp;/gi, " ")
      .replace(/&lt;/gi, "<")
      .replace(/&gt;/gi, ">")
      .replace(/&quot;/gi, '"')
      .replace(/&apos;/gi, "'")
      .replace(/&amp;/gi, "&");
  }

  function stripHtml(html) {
    var text = String(html == null ? "" : html)
      .replace(/<script[\s\S]*?<\/script>/gi, " ")
      .replace(/<style[\s\S]*?<\/style>/gi, " ")
      .replace(/<[^>]*>/g, " ");
    return decodeHtml(text).replace(/\s+/g, " ").trim();
  }

  function isChaoxingUrl(value) {
    return /^https:\/\/([a-z0-9-]+\.)*chaoxing\.com(:\d+)?([\/?#]|$)/i.test(
      String(value || ""),
    );
  }

  function absoluteUrl(base, location) {
    var loc = String(location || "").trim();
    if (/^https:\/\//i.test(loc)) return loc;
    if (/^http:\/\//i.test(loc)) return "";
    if (loc.indexOf("//") === 0) return "https:" + loc;
    var originMatch = /^https:\/\/[^\/?#]+/i.exec(base);
    if (!originMatch) return "";
    var origin = originMatch[0];
    if (loc.charAt(0) === "/") return origin + loc;
    var withoutHash = base.split("#")[0].split("?")[0];
    var slash = withoutHash.lastIndexOf("/");
    return (slash >= 0 ? withoutHash.slice(0, slash + 1) : origin + "/") + loc;
  }

  function queryParam(url, name) {
    var pattern = new RegExp("[?&]" + name + "=([^&#]*)", "i");
    var match = pattern.exec(String(url || ""));
    if (!match) return null;
    try {
      return decodeURIComponent(match[1]);
    } catch (error) {
      return match[1];
    }
  }

  function hashKey(value) {
    var hash = 0;
    var text = String(value || "");
    for (var i = 0; i < text.length; i += 1) {
      hash = (hash * 31 + text.charCodeAt(i)) | 0;
    }
    return String(hash >>> 0);
  }

  function addWarning(warnings, message) {
    if (warnings.length < MAX_WARNINGS) warnings.push(clean(message, 200));
  }

  function looksLoggedOut(page) {
    if (
      String(page.url || "").indexOf("passport2.chaoxing.com/login") >= 0 ||
      String(page.url || "").indexOf("/fanyalogin") >= 0
    ) {
      return true;
    }
    var body = String(page.body || "");
    return (
      /<title>\s*用户登录\s*<\/title>/i.test(body) ||
      /id=["']loginBtn["']/i.test(body)
    );
  }

  function requestPage(ctx, url, referer) {
    var current = url;
    var currentReferer = referer || url;
    for (var hop = 0; hop < 5; hop += 1) {
      var headers = {
        Accept:
          "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8",
        "Accept-Language": "zh-CN,zh;q=0.9",
      };
      if (currentReferer) headers.Referer = currentReferer;
      var response = ctx.http.request({
        url: current,
        method: "GET",
        headers: headers,
      });
      if (response.error) {
        throw new ApiError("NETWORK", response.error);
      }
      if (response.status === 401 || response.status === 403) {
        throw new ApiError("AUTH_EXPIRED", "HTTP " + response.status);
      }
      if (response.status === 429) {
        throw new ApiError("RATE_LIMITED", "HTTP 429");
      }
      if (response.status >= 300 && response.status < 400) {
        var location = response.location || "";
        if (!location) {
          throw new ApiError("AUTH_EXPIRED", "redirect without location");
        }
        var next = absoluteUrl(current, location);
        if (!next || !isChaoxingUrl(next)) {
          throw new ApiError("AUTH_EXPIRED", "redirected outside chaoxing");
        }
        currentReferer = current;
        current = next;
        continue;
      }
      if (response.status < 200 || response.status >= 300) {
        throw new ApiError("HTTP_ERROR", "HTTP " + response.status);
      }
      var page = { url: current, body: String(response.body || "") };
      if (looksLoggedOut(page)) {
        throw new ApiError("AUTH_EXPIRED", "session expired");
      }
      if (/antispiderShowVerify\.ac|processVerifyPng\.ac/i.test(page.body)) {
        throw new ApiError("RISK_CONTROL", "platform risk control");
      }
      return page;
    }
    throw new ApiError("AUTH_EXPIRED", "too many redirects");
  }

  function beijingMs(year, month, day, hour, minute) {
    return Date.UTC(year, month - 1, day, hour, minute, 0, 0) - BEIJING_OFFSET_MS;
  }

  function inferYearMs(month, day, hour, minute, now) {
    var beijingNow = new Date(now + BEIJING_OFFSET_MS);
    var year = beijingNow.getUTCFullYear();
    var best = null;
    var bestDiff = Number.MAX_SAFE_INTEGER;
    for (var offset = -1; offset <= 1; offset += 1) {
      var candidate = beijingMs(year + offset, month, day, hour, minute);
      var diff = Math.abs(candidate - now);
      if (diff < bestDiff) {
        best = candidate;
        bestDiff = diff;
      }
    }
    return best;
  }

  function parseDateMs(text, now) {
    var value = String(text || "");
    var full = /(\d{4})\s*[-\/年]\s*(\d{1,2})\s*[-\/月]\s*(\d{1,2})\s*日?\s*(\d{1,2})\s*[:：]\s*(\d{2})/.exec(
      value,
    );
    if (full) {
      return beijingMs(
        parseInt(full[1], 10),
        parseInt(full[2], 10),
        parseInt(full[3], 10),
        parseInt(full[4], 10),
        parseInt(full[5], 10),
      );
    }
    var short = /(\d{1,2})\s*[-\/月]\s*(\d{1,2})\s*日?\s*(\d{1,2})\s*[:：]\s*(\d{2})/.exec(
      value,
    );
    if (short) {
      return inferYearMs(
        parseInt(short[1], 10),
        parseInt(short[2], 10),
        parseInt(short[3], 10),
        parseInt(short[4], 10),
        now,
      );
    }
    return null;
  }

  function parseRemainingMs(text) {
    var value = String(text || "");
    var days = /(\d+)\s*天/.exec(value);
    var hours = /(\d+)\s*(?:小时|时)/.exec(value);
    var minutes = /(\d+)\s*分/.exec(value);
    var seconds = /(\d+)\s*秒/.exec(value);
    if (!days && !hours && !minutes && !seconds) return null;
    return (
      (days ? parseInt(days[1], 10) : 0) * 86400000 +
      (hours ? parseInt(hours[1], 10) : 0) * 3600000 +
      (minutes ? parseInt(minutes[1], 10) : 0) * 60000 +
      (seconds ? parseInt(seconds[1], 10) : 0) * 1000
    );
  }

  function isCompletedStatus(status) {
    return /已完成|已提交|待批阅|已批阅|已批改|已交/.test(String(status || ""));
  }

  function examListUrl(fid) {
    return EXAM_LIST + encodeURIComponent(fid || "");
  }

  function loadCourses(ctx) {
    var urls = [
      COURSE_LIST,
      "https://mooc1-api.chaoxing.com/mycourse/backclazzdata?view=json&mcode=",
      "https://mooc1-api.chaoxing.com/mycourse/backclazzdata",
    ];
    var lastError = new ApiError("SCHEMA_CHANGED", "课程列表不可用");
    for (var u = 0; u < urls.length; u += 1) {
      var page;
      try {
        page = requestPage(ctx, urls[u], "https://mooc1-api.chaoxing.com/");
      } catch (error) {
        if (
          error &&
          (error.code === "AUTH_EXPIRED" || error.code === "RATE_LIMITED")
        ) {
          throw error;
        }
        lastError = error;
        continue;
      }
      var data;
      try {
        data = JSON.parse(page.body);
      } catch (error) {
        lastError = new ApiError("SCHEMA_CHANGED", "课程列表不是 JSON");
        continue;
      }
      var channels = data && data.channelList;
      if (!Array.isArray(channels)) {
        lastError = new ApiError("SCHEMA_CHANGED", "课程列表缺少 channelList");
        continue;
      }
      var courses = [];
      for (var i = 0; i < channels.length; i += 1) {
        var content = (channels[i] && channels[i].content) || {};
        var clazzId = asId(content.id);
        var courseData = content.course && content.course.data;
        var first = Array.isArray(courseData) ? courseData[0] : null;
        var courseId = asId(first && first.id);
        if (clazzId === null || courseId === null) continue;
        courses.push({
          courseId: courseId,
          clazzId: clazzId,
          cpi: asId(content.cpi),
          name: clean(
            (first && first.name) || content.name || "未命名课程",
            120,
          ),
        });
      }
      return courses;
    }
    throw lastError;
  }

  function findCourse(courses, courseId, clazzId) {
    for (var i = 0; i < courses.length; i += 1) {
      if (
        (courseId !== null && courses[i].courseId === courseId) ||
        (clazzId !== null && courses[i].clazzId === clazzId)
      ) {
        return courses[i];
      }
    }
    return null;
  }

  function courseUrl(course) {
    if (!course) return "https://i.chaoxing.com/base";
    // `stucoursemiddle` is the stable course entrance; it redirects to the
    // course home and does not need the time-bound page `enc`.
    return (
      "https://mooc1-2.chaoxing.com/visit/stucoursemiddle?courseid=" +
      encodeURIComponent(course.courseId) +
      "&clazzid=" +
      encodeURIComponent(course.clazzId) +
      "&vc=1" +
      (course.cpi ? "&cpi=" + encodeURIComponent(course.cpi) : "")
    );
  }

  function parseWorkItems(html, pageUrl, courses, now) {
    var items = [];
    var rowPattern = /<li([^>]*\bdata=["']([^"']+)["'][^>]*)>([\s\S]{0,8000}?)<\/li>/gi;
    var match;
    while ((match = rowPattern.exec(String(html || ""))) !== null) {
      var openingTag = match[1];
      var dataUrl = decodeHtml(match[2]);
      var body = match[3];
      var ariaMatch =
        /aria-label=["']([^"']*)["']/i.exec(openingTag) ||
        /aria-label=["']([^"']*)["']/i.exec(body);
      var aria = ariaMatch ? decodeHtml(ariaMatch[1]) : "";

      var titleMatch = /作业名称[：:]?\s*([\s\S]*?)作业状态/.exec(aria);
      var statusMatch = /作业状态[：:]?\s*([\s\S]*?)所属课程/.exec(aria);
      var courseMatch = /所属课程[：:]?\s*《?([^》]*?)》?(?=\s*(?:[;；]|剩余时间|$))/.exec(
        aria,
      );
      var remainingMatch = /剩余时间[：:]?\s*([^;；"'<]*)/.exec(aria);

      var title = titleMatch ? clean(titleMatch[1], 300) : "";
      var status = statusMatch ? clean(statusMatch[1], 120) : "";
      var courseName = courseMatch ? clean(courseMatch[1], 120) : "";

      if (!title) {
        var titleHtml = /<p[^>]*class=["'][^"']*\boverHidden2\b[^"']*["'][^>]*>([\s\S]*?)<\/p>/i.exec(
          body,
        );
        if (titleHtml) title = clean(stripHtml(titleHtml[1]), 300);
      }
      if (!status) {
        var statusHtml = /<p[^>]*class=["'][^"']*\bstatus\b[^"']*["'][^>]*>([\s\S]*?)<\/p>/i.exec(
          body,
        );
        if (statusHtml) status = clean(stripHtml(statusHtml[1]), 120);
      }
      if (!title) continue;

      var href = absoluteUrl(pageUrl, dataUrl);
      if (href && !isChaoxingUrl(href)) href = "";
      var courseId = asId(queryParam(href, "courseId") || queryParam(href, "courseid"));
      var clazzId = asId(queryParam(href, "classId") || queryParam(href, "classid"));
      var course = findCourse(courses, courseId, clazzId);
      if (!courseName) courseName = course ? course.name : "";

      var workId =
        asId(queryParam(href, "workId")) ||
        asId(queryParam(href, "workid")) ||
        asId(queryParam(href, "taskrefId")) ||
        asId(queryParam(href, "id"));
      var remoteId = workId
        ? "work:" + (courseId || (course ? course.courseId : "0")) + ":" + workId
        : "work:" + hashKey((courseId || "") + "|" + title + "|" + href);

      var remainingMs = parseRemainingMs(
        remainingMatch ? remainingMatch[1] : "",
      );
      if (remainingMs === null) {
        var remainingHtml = /<div[^>]*class=["'][^"']*\btime\b[^"']*["'][^>]*>([\s\S]*?)<\/div>/i.exec(
          body,
        );
        if (remainingHtml) {
          remainingMs = parseRemainingMs(stripHtml(remainingHtml[1]));
        }
      }
      var dueAt = remainingMs === null ? null : now + remainingMs;
      var completed = isCompletedStatus(status);

      items.push({
        remoteId: remoteId,
        title: title,
        course: courseName,
        kind: "assignment",
        dueAt: dueAt,
        startsAt: null,
        status: status || "pending",
        completed: completed,
        // The detail URL carries a time-bound `enc`, which is unlikely to be
        // valid in the system browser. Point the task card at the stable
        // course entrance; the detail URL is only used for parsing.
        url: courseUrl(course),
        detailUrl: href,
      });
    }
    return items;
  }

  function parseWorkDetail(html, now) {
    var text = stripHtml(html);
    var result = {
      startsAt: null,
      dueAt: null,
      status: "",
      yearInferred: false,
    };
    var rangePattern = /(?:作答时间|作答题时间|作业时间)[：:]\s*((?:\d{4}\s*[-\/年]\s*)?\d{1,2}\s*[-\/月]\s*\d{1,2}\s*日?\s*\d{1,2}\s*[:：]\s*\d{2})\s*(?:至|~|到)\s*((?:\d{4}\s*[-\/年]\s*)?\d{1,2}\s*[-\/月]\s*\d{1,2}\s*日?\s*\d{1,2}\s*[:：]\s*\d{2})/.exec(
      text,
    );
    if (rangePattern) {
      result.startsAt = parseDateMs(rangePattern[1], now);
      result.dueAt = parseDateMs(rangePattern[2], now);
      result.yearInferred = !/\d{4}/.test(rangePattern[2]);
    }
    if (result.dueAt === null) {
      var singlePattern = /(?:提交截止时间|截止时间|结束时间|作业截止时间)[：:]\s*((?:\d{4}\s*[-\/年]\s*)?\d{1,2}\s*[-\/月]\s*\d{1,2}\s*日?\s*\d{1,2}\s*[:：]\s*\d{2})/.exec(
        text,
      );
      if (singlePattern) {
        result.dueAt = parseDateMs(singlePattern[1], now);
        result.yearInferred = !/\d{4}/.test(singlePattern[1]);
      }
    }
    var statusPattern = /(?:作业状态|状态)[：:]\s*(未提交|已完成|已提交|待批阅|已批阅|已批改|已交)/.exec(
      text,
    );
    if (statusPattern) result.status = statusPattern[1];
    return result;
  }

  function shiftBeijingYear(ms, delta) {
    var beijing = new Date(ms + BEIJING_OFFSET_MS);
    return beijingMs(
      beijing.getUTCFullYear() + delta,
      beijing.getUTCMonth() + 1,
      beijing.getUTCDate(),
      beijing.getUTCHours(),
      beijing.getUTCMinutes(),
    );
  }

  function enrichWorkDetails(ctx, items, warnings, now) {
    var pending = [];
    for (var i = 0; i < items.length && pending.length < MAX_DETAIL_REQUESTS; i += 1) {
      if (!items[i].completed && items[i].detailUrl) pending.push(items[i]);
    }
    var started = Date.now();
    for (var p = 0; p < pending.length; p += 1) {
      // Exact deadlines are nice-to-have. Keep the whole adapter run bounded
      // so a large account cannot exceed the host's execution timeout.
      if (Date.now() - started > 45000) break;
      var item = pending[p];
      try {
        var page = requestPage(ctx, item.detailUrl, WORK_AGGREGATE);
        var detail = parseWorkDetail(page.body, now);
        // Yearless dates pick the nearest year. If the aggregate's relative
        // countdown says the deadline is still ahead but the inferred date is
        // already past, the detail belongs to the next year.
        if (
          detail.yearInferred &&
          detail.dueAt !== null &&
          detail.dueAt < now &&
          item.dueAt !== null &&
          item.dueAt > now
        ) {
          var shiftedDue = shiftBeijingYear(detail.dueAt, 1);
          if (shiftedDue > now) {
            if (detail.startsAt !== null) {
              detail.startsAt = shiftBeijingYear(detail.startsAt, 1);
            }
            detail.dueAt = shiftedDue;
          }
        }
        if (detail.startsAt !== null) item.startsAt = detail.startsAt;
        if (detail.dueAt !== null) item.dueAt = detail.dueAt;
        if (detail.status) {
          item.status = detail.status;
          item.completed = isCompletedStatus(detail.status);
        }
      } catch (error) {
        if (
          error &&
          (error.code === "AUTH_EXPIRED" || error.code === "RATE_LIMITED")
        ) {
          throw error;
        }
        addWarning(warnings, item.title + " 截止时间解析失败");
      }
      ctx.sleep(DETAIL_GAP_MS);
    }
  }

  function parseExamTable(html, fid, now) {
    var tasks = [];
    var rowPattern = /<tr[^>]*class=["'][^"']*\bdataTr\b[^"']*["'][^>]*>([\s\S]*?)<\/tr>/gi;
    var row;
    while ((row = rowPattern.exec(String(html || ""))) !== null) {
      var rowHtml = row[1];
      var cells = [];
      var cellPattern = /<td[^>]*>([\s\S]*?)<\/td>/gi;
      var cell;
      while ((cell = cellPattern.exec(rowHtml)) !== null) {
        cells.push(clean(stripHtml(cell[1]), 500));
      }
      if (cells.length < 3) continue;
      var title = clean(cells[1], 300);
      if (!title) continue;
      var timeText = cells[2] || "";
      var rangePattern = /(\d{4})-(\d{1,2})-(\d{1,2})\s+(\d{1,2}):(\d{2})\s*(?:至|~|到)\s*(\d{4})-(\d{1,2})-(\d{1,2})\s+(\d{1,2}):(\d{2})/.exec(
        timeText,
      );
      var startsAt = null;
      var dueAt = null;
      if (rangePattern) {
        startsAt = beijingMs(
          parseInt(rangePattern[1], 10),
          parseInt(rangePattern[2], 10),
          parseInt(rangePattern[3], 10),
          parseInt(rangePattern[4], 10),
          parseInt(rangePattern[5], 10),
        );
        dueAt = beijingMs(
          parseInt(rangePattern[6], 10),
          parseInt(rangePattern[7], 10),
          parseInt(rangePattern[8], 10),
          parseInt(rangePattern[9], 10),
          parseInt(rangePattern[10], 10),
        );
      } else {
        var sameDayRange = /(\d{4})-(\d{1,2})-(\d{1,2})\s+(\d{1,2}):(\d{2})\s*(?:至|~|到|-)\s*(\d{1,2}):(\d{2})/.exec(
          timeText,
        );
        if (sameDayRange) {
          startsAt = beijingMs(
            parseInt(sameDayRange[1], 10),
            parseInt(sameDayRange[2], 10),
            parseInt(sameDayRange[3], 10),
            parseInt(sameDayRange[4], 10),
            parseInt(sameDayRange[5], 10),
          );
          dueAt = beijingMs(
            parseInt(sameDayRange[1], 10),
            parseInt(sameDayRange[2], 10),
            parseInt(sameDayRange[3], 10),
            parseInt(sameDayRange[6], 10),
            parseInt(sameDayRange[7], 10),
          );
        } else {
          dueAt = parseDateMs(timeText, now);
        }
      }
      if (dueAt === null) continue;
      var status = clean(cells[5] || cells[4] || "pending", 120);
      var examIdMatch =
        /examId\s*[=:]\s*["']?(\d+)/i.exec(rowHtml) ||
        /examid\s*[=:]\s*["']?(\d+)/i.exec(rowHtml);
      var remoteId = examIdMatch
        ? "exam:" + examIdMatch[1]
        : "exam:" + hashKey(title + "|" + startsAt + "|" + dueAt);
      tasks.push({
        remoteId: remoteId,
        title: title,
        // The school-wide exam table has no course column. The title is the
        // only safe display fallback; do not invent a course.
        course: clean(title, 120),
        kind: "exam",
        dueAt: dueAt,
        startsAt: startsAt,
        status: status,
        completed: isCompletedStatus(status),
        url: examListUrl(fid),
      });
    }
    return tasks;
  }

  function parseExamAggregate(html, fid, now) {
    var tasks = [];
    var rowPattern = /<li([^>]*\bdata=["']([^"']+)["'][^>]*)>([\s\S]{0,5000}?)<\/li>/gi;
    var row;
    while ((row = rowPattern.exec(String(html || ""))) !== null) {
      var openingTag = row[1];
      var dataUrl = decodeHtml(row[2]);
      var body = row[3];
      var ariaMatch =
        /aria-label=["']([^"']*)["']/i.exec(openingTag) ||
        /aria-label=["']([^"']*)["']/i.exec(body);
      var aria = ariaMatch ? decodeHtml(ariaMatch[1]) : "";
      var titleMatch = /考试名称[：:]\s*([^;；]*)/.exec(aria);
      var statusMatch = /考试状态[：:]\s*([^;；]*)/.exec(aria);
      var remainingMatch = /剩余时间[：:]\s*([^;；<]*)/.exec(aria);
      var title = titleMatch ? clean(titleMatch[1], 300) : "";
      if (!title) {
        var titleHtml = /<dt[^>]*>([\s\S]*?)<\/dt>/i.exec(body);
        if (titleHtml) title = clean(stripHtml(titleHtml[1]), 300);
      }
      if (!title) continue;
      var remainingMs = parseRemainingMs(
        remainingMatch ? remainingMatch[1] : "",
      );
      if (remainingMs === null) continue;
      var dueAt = now + remainingMs;
      var status = statusMatch ? clean(statusMatch[1], 120) : "pending";
      var href = absoluteUrl("https://mooc1-api.chaoxing.com/", dataUrl);
      var examId =
        asId(queryParam(href, "examId")) ||
        asId(queryParam(href, "examid")) ||
        asId(queryParam(href, "taskrefId"));
      var remoteId = examId
        ? "exam:" + examId
        : "exam:" + hashKey(title + "|" + dueAt);
      tasks.push({
        remoteId: remoteId,
        title: title,
        course: clean(title, 120),
        kind: "exam",
        dueAt: dueAt,
        startsAt: null,
        status: status,
        completed: isCompletedStatus(status),
        url: examListUrl(fid),
      });
    }
    return tasks;
  }

  function mergeTask(seen, task) {
    if (seen.byId[task.remoteId]) return;
    // Work items are deduped by their stable work id only; two distinct
    // assignments may legitimately share a title and deadline. Exams also
    // overlap between the table and the phone aggregate with different ids,
    // so use a title/time key as a second guard there.
    if (task.kind === "exam") {
      var key =
        "exam|" +
        clean(task.title, 300).toLowerCase() +
        "|" +
        (task.startsAt === null ? 0 : task.startsAt) +
        "|" +
        (task.dueAt === null ? 0 : task.dueAt);
      if (seen.byKey[key]) {
        var existing = seen.byKey[key];
        if (existing.startsAt === null && task.startsAt !== null) {
          existing.startsAt = task.startsAt;
        }
        if (existing.dueAt === null && task.dueAt !== null) {
          existing.dueAt = task.dueAt;
        }
        return;
      }
      seen.byKey[key] = task;
    }
    seen.byId[task.remoteId] = true;
    seen.tasks.push(task);
  }

  function failure(error, tasks, warnings) {
    var code = error && error.code ? error.code : "RUNTIME_ERROR";
    var result = {
      tasks: tasks || [],
      complete: false,
      warnings: (warnings || []).slice(0, MAX_WARNINGS),
      error: code,
    };
    if (error && error.message) {
      addWarning(result.warnings, String(error.message));
    }
    return result;
  }

  function fetchTasks(ctx) {
    var now = Date.now();
    var warnings = [];
    var seen = { byId: {}, byKey: {}, tasks: [] };
    var complete = true;
    var fid = "";

    // 1. Validate the session and read the school fid. This is the only call
    // whose failure is fatal: everything else can degrade to a partial sync.
    var basePage;
    try {
      basePage = requestPage(ctx, BASE_PAGE, "https://i.chaoxing.com/");
    } catch (error) {
      return failure(error, [], warnings);
    }
    var fidMatch =
      /"fid"\s*:\s*"?(\d+)"?/i.exec(basePage.body) ||
      /var\s+fid\s*=\s*["']?(\d+)/i.exec(basePage.body);
    var cookieFid = ctx.session.get("fid") || "";
    if (cookieFid === "-1" || cookieFid === "0") cookieFid = "";
    fid = (fidMatch && fidMatch[1]) || cookieFid;

    // 2. Course list is used for names and course-page links. Failure is not
    // fatal because the aggregate work/exam sources carry their own metadata.
    var courses = [];
    try {
      courses = loadCourses(ctx);
    } catch (error) {
      if (
        error &&
        (error.code === "AUTH_EXPIRED" || error.code === "RATE_LIMITED")
      ) {
        return failure(error, seen.tasks, warnings);
      }
      addWarning(warnings, "课程列表拉取失败");
    }
    ctx.sleep(SOURCE_GAP_MS);

    // 3. Aggregate pending work (no enc/token bootstrap needed). Exact
    // deadlines come from each item's own detail page, bounded to avoid a
    // request storm; relative "剩余时间" remains as a fallback.
    try {
      var workPage = requestPage(ctx, WORK_AGGREGATE, "https://mooc1-api.chaoxing.com/");
      var workItems = parseWorkItems(workPage.body, workPage.url, courses, now);
      if (
        workItems.length === 0 &&
        !/<ul[\s>]|<nav[\s>]|class=["'][^"']*\bnav\b/i.test(workPage.body)
      ) {
        // An empty page without the expected list shell is more likely a
        // schema/anti-bot page than "no pending work". Keep the old snapshot.
        complete = false;
        addWarning(warnings, "作业列表为空且结构异常");
      }
      enrichWorkDetails(ctx, workItems, warnings, now);
      for (var w = 0; w < workItems.length; w += 1) {
        mergeTask(seen, workItems[w]);
      }
    } catch (error) {
      if (
        error &&
        (error.code === "AUTH_EXPIRED" || error.code === "RATE_LIMITED")
      ) {
        return failure(error, seen.tasks, warnings);
      }
      complete = false;
      addWarning(warnings, "作业列表拉取失败");
    }
    ctx.sleep(SOURCE_GAP_MS);

    // 4. School-wide exam table: absolute start/end times in one request.
    // Fall back to the phone aggregate (relative times) when empty.
    var examsOk = true;
    try {
      var examPage = requestPage(ctx, examListUrl(fid), "https://mooc1.chaoxing.com/");
      var examTableLooksValid =
        /class=["'][^"']*\bdataTable\b/i.test(examPage.body) ||
        /\bdataTr\b/i.test(examPage.body);
      var examTasks = parseExamTable(examPage.body, fid, now);
      if (examTasks.length === 0) {
        try {
          var fallbackPage = requestPage(
            ctx,
            EXAM_AGGREGATE,
            "https://mooc1-api.chaoxing.com/",
          );
          examTasks = parseExamAggregate(fallbackPage.body, fid, now);
        } catch (fallbackError) {
          if (
            fallbackError &&
            (fallbackError.code === "AUTH_EXPIRED" ||
              fallbackError.code === "RATE_LIMITED")
          ) {
            throw fallbackError;
          }
          addWarning(warnings, "考试备用列表拉取失败");
        }
      }
      if (examTasks.length === 0 && !examTableLooksValid) {
        examsOk = false;
        addWarning(warnings, "考试列表为空且结构异常");
      }
      for (var e = 0; e < examTasks.length; e += 1) {
        mergeTask(seen, examTasks[e]);
      }
    } catch (error) {
      if (
        error &&
        (error.code === "AUTH_EXPIRED" || error.code === "RATE_LIMITED")
      ) {
        return failure(error, seen.tasks, warnings);
      }
      examsOk = false;
      addWarning(warnings, "考试列表拉取失败");
    }
    if (!examsOk) complete = false;

    seen.tasks.sort(function (left, right) {
      var leftTarget =
        left.startsAt || left.dueAt || Number.MAX_SAFE_INTEGER;
      var rightTarget =
        right.startsAt || right.dueAt || Number.MAX_SAFE_INTEGER;
      return leftTarget - rightTarget;
    });
    ctx.log.info(
      "chaoxing adapter fetched " +
        seen.tasks.length +
        " tasks (" +
        courses.length +
        " courses)",
    );
    return {
      tasks: seen.tasks,
      complete: complete,
      warnings: warnings,
    };
  }

  globalThis.adapter = { fetchTasks: fetchTasks };
})();
