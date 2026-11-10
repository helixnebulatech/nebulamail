// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

// HTTP-date (RFC 9110 §5.6.7). Self-contained, no dependencies: also used
// by the image proxy for `Expires` (subpath `@nebulamail/jmap-client/http-date`).
//
//   IMF-fixdate  Sun, 06 Nov 1994 08:49:37 GMT   (the one senders use)
//   rfc850-date  Sunday, 06-Nov-94 08:49:37 GMT  (obsolete)
//   asctime-date Sun Nov  6 08:49:37 1994        (obsolete)
//
// "A recipient that parses a timestamp value in an HTTP field MUST accept
// all three HTTP-date formats." `Date.parse` alone is not used: it reads
// odd things ("-1", "0") as dates.

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

const DAY = "(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun)";
const DAY_LONG = "(?:Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)";
const MONTH = `(?<month>${MONTHS.join("|")})`;
const TIME = String.raw`(?<hour>\d{2}):(?<minute>\d{2}):(?<second>\d{2})`;

const IMF_FIXDATE = new RegExp(
  String.raw`^${DAY}, (?<day>\d{2}) ${MONTH} (?<year>\d{4}) ${TIME} GMT$`,
  "u"
);
const RFC850_DATE = new RegExp(
  String.raw`^${DAY_LONG}, (?<day>\d{2})-${MONTH}-(?<year>\d{2}) ${TIME} GMT$`,
  "u"
);
// date3 = month SP ( 2DIGIT / ( SP DIGIT ) )
const ASCTIME_DATE = new RegExp(
  String.raw`^${DAY} ${MONTH} (?<day>\d{2}| \d) ${TIME} (?<year>\d{4})$`,
  "u"
);

/**
 * rfc850-date has a two-digit year. RFC 9110 §5.6.7: a value that "appears
 * to be more than 50 years in the future" is "the most recent year in the
 * past that had the same last two digits".
 */
const fullYear = (twoDigits: number, now: number): number => {
  const current = new Date(now).getUTCFullYear();
  let year = Math.floor(current / 100) * 100 + twoDigits;
  if (year > current + 50) {
    year -= 100;
  }
  return year;
};

/**
 * Milliseconds since the epoch of an HTTP-date, or null if `value` is not
 * one (or names a day that doesn't exist, like 31 Feb). The weekday is
 * not checked against the date (RFC 9110 doesn't ask recipients to).
 */
export const parseHttpDate = (
  value: string,
  now: number = Date.now()
): number | null => {
  const trimmed = value.trim();
  const imf = IMF_FIXDATE.exec(trimmed);
  const rfc850 = imf === null ? RFC850_DATE.exec(trimmed) : null;
  const asctime =
    imf === null && rfc850 === null ? ASCTIME_DATE.exec(trimmed) : null;
  const groups = (imf ?? rfc850 ?? asctime)?.groups;
  if (groups === undefined) {
    return null;
  }
  const number = (name: string): number => Number(groups[name]?.trim());
  const year = rfc850 === null ? number("year") : fullYear(number("year"), now);
  const month = MONTHS.indexOf(
    // The regex only matches these names.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion
    groups.month as (typeof MONTHS)[number]
  );
  const day = number("day");
  const hour = number("hour");
  const minute = number("minute");
  // RFC 9110 §5.6.7: second is 00-60 (a leap second).
  const second = number("second");
  if (hour > 23 || minute > 59 || second > 60) {
    return null;
  }
  // setUTCFullYear: Date.UTC would read years 0-99 as 1900-1999.
  const date = new Date(0);
  date.setUTCFullYear(year, month, day);
  date.setUTCHours(hour, minute, Math.min(second, 59));
  if (date.getUTCDate() !== day || date.getUTCMonth() !== month) {
    return null;
  }
  return date.getTime() + (second === 60 ? 1000 : 0);
};
