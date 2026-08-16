export const RUNTIME: "napi";

export type EmbeddingOptions = {
  provider?: string;
  model?: string;
  dimensions?: number;
};

export type OpenOptions = {
  embedding?: EmbeddingOptions;
};

export type QueryResult = {
  columns: string[];
  rows: Array<Array<string | number | null>>;
};

export type DocumentInsert = {
  title?: string;
  content: string;
  metadata?: Record<string, unknown>;
};

export type AgentRun = {
  instructions: string;
  goal: string;
  tools?: string[];
  maxSteps?: number;
  k?: number;
  memory?: string;
  agents?: AgentRun[];
  decide?: boolean;
  session?: string;
};

export declare class Database {
  readonly path: string;
  query(sql: string): Promise<QueryResult>;
  execute(sql: string): Promise<number>;
  session(name?: string | null): Promise<string>;
  lastRunId(): Promise<string>;
  readonly documents: {
    insert(doc: DocumentInsert): Promise<{ id: string }>;
  };
  readonly memory: {
    insert(doc: {
      content: string;
      scope?: string;
      userId?: string;
    }): Promise<{ id: string }>;
    search(options: {
      query: string;
      scope?: string;
      userId?: string;
      limit?: number;
    }): Promise<QueryResult>;
  };
  search(
    query: string,
    options?: { limit?: number }
  ): Promise<QueryResult>;
  readonly agent: {
    run(spec: AgentRun): Promise<{ run_id: string; status: string; output: string }>;
  };
  readonly runs: {
    waiting(): Promise<QueryResult>;
    resume(
      id: string,
      decision?: { approved?: boolean }
    ): Promise<{ run_id: string; status: string; output: string }>;
    events(id: string): Promise<QueryResult>;
    tokens(id: string): Promise<QueryResult>;
  };
  close(): Promise<void>;
}

export type TokenEvent = {
  runId: string;
  seq: number;
  text: string;
};

export declare class AI {
  static readonly runtime: "napi";
  static subscribeTokens(callback: (event: TokenEvent) => void): void;
  static open(path: string, options?: OpenOptions): Promise<Database>;
}
