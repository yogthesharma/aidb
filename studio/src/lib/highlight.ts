const SQL_KEYWORDS = new Set(
  [
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "as",
    "on",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "group",
    "by",
    "order",
    "limit",
    "offset",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "view",
    "index",
    "model",
    "key_name",
    "using",
    "with",
    "distinct",
    "union",
    "all",
    "case",
    "when",
    "then",
    "else",
    "end",
    "in",
    "is",
    "null",
    "like",
    "between",
    "exists",
    "pragma",
    "count",
    "substr",
    "coalesce",
    "length",
    "desc",
    "asc",
  ].map((word) => word.toUpperCase()),
);

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

function wrap(kind: string, value: string): string {
  return `<span class="tok-${kind}">${value}</span>`;
}

export function highlightSql(source: string): string {
  const escaped = escapeHtml(source);
  return escaped.replace(
    /(--[^\n]*)|(\/\*[\s\S]*?\*\/)|('(?:''|[^'])*')|("(?:""|[^"])*")|(\b\d+(?:\.\d+)?\b)|(\b[A-Za-z_][A-Za-z0-9_]*\b)/g,
    (
      match,
      commentLine?: string,
      commentBlock?: string,
      single?: string,
      ident?: string,
      number?: string,
      word?: string,
    ) => {
      if (commentLine || commentBlock) {
        return wrap("comment", match);
      }
      if (single) {
        return wrap("string", match);
      }
      if (ident) {
        return wrap("ident", match);
      }
      if (number) {
        return wrap("number", match);
      }
      if (word) {
        if (word.toLowerCase().startsWith("aidb_")) {
          return wrap("fn", match);
        }
        if (SQL_KEYWORDS.has(word.toUpperCase())) {
          return wrap("kw", match);
        }
      }
      return match;
    },
  );
}

export function highlightJson(source: string): string {
  const escaped = escapeHtml(source);
  return escaped.replace(
    /("(?:\\.|[^"\\])*")\s*:|("(?:\\.|[^"\\])*")|(\btrue\b|\bfalse\b|\bnull\b)|(-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b)/g,
    (
      match,
      key?: string,
      string?: string,
      literal?: string,
      number?: string,
    ) => {
      if (key) {
        return `${wrap("key", key)}:`;
      }
      if (string) {
        return wrap("string", string);
      }
      if (literal) {
        return wrap("kw", literal);
      }
      if (number) {
        return wrap("number", number);
      }
      return match;
    },
  );
}
