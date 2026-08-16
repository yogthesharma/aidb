export const PAGE_SEGMENT: {
  overview: "file";
  sql: "sql";
  documents: "documents";
  search: "search";
  runs: "runs";
  models: "models";
  experiments: "experiments";
};

export const CATALOG_SQL: {
  meta: string;
  tables: string;
  documents: string;
  runs: string;
  models: string;
  experiments: string;
  nDocuments: string;
  nRuns: string;
  nModels: string;
  nWaiting: string;
  nExperiments: string;
  sessions: string;
  sessionTurns: string;
  tokens: string;
};

export function sqlString(value: string): string;
export function searchSql(query: string, k: number | string): string;
export function resumeSql(runId: string, approved: boolean): string;
