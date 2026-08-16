// The desk's research corpus. Short on purpose: every fact here is checkable, so a
// cited answer can be verified by reading the source it points at.

export const WATCHLIST = [
  { ticker: "AAPL", name: "Apple Inc." },
  { ticker: "MSFT", name: "Microsoft Corporation" },
  { ticker: "NVDA", name: "NVIDIA Corporation" },
];

// `sourceId` is the desk's own key for a filing. It goes into the document's
// metadata so re-ingesting the same corpus is a no-op.
export const CORPUS = [
  {
    sourceId: "aapl-10k-fy2024",
    title: "AAPL 10-K excerpt (capital return)",
    ticker: "AAPL",
    kind: "filing",
    period: "FY2024",
    content:
      "Apple returned 110 billion dollars to shareholders through share repurchases in fiscal 2024. " +
      "Services revenue reached 96 billion dollars and grew 13 percent year over year. " +
      "The company held 65 billion dollars in net cash at year end.",
  },
  {
    sourceId: "aapl-call-q1fy2025",
    title: "AAPL earnings call (December quarter guidance)",
    ticker: "AAPL",
    kind: "call",
    period: "Q1FY2025",
    content:
      "Management guided to low single digit revenue growth for the December quarter. " +
      "Gross margin is expected to land between 46 and 47 percent. " +
      "Foreign exchange is a two point headwind to reported growth.",
  },
  {
    sourceId: "msft-10k-fy2024",
    title: "MSFT 10-K excerpt (commercial bookings)",
    ticker: "MSFT",
    kind: "filing",
    period: "FY2024",
    content:
      "Microsoft commercial bookings grew 17 percent in constant currency. " +
      "Commercial remaining performance obligation stood at 269 billion dollars. " +
      "Azure capacity constraints are expected to ease in the second half of the fiscal year.",
  },
  {
    sourceId: "msft-call-q1fy2025",
    title: "MSFT earnings call (capital expenditure)",
    ticker: "MSFT",
    kind: "call",
    period: "Q1FY2025",
    content:
      "Capital expenditure will increase sequentially as AI infrastructure comes online. " +
      "Roughly half of cloud spend is long lived assets that will be monetized over 15 years. " +
      "Operating margin is expected to be roughly flat year over year.",
  },
  {
    sourceId: "nvda-10k-fy2025",
    title: "NVDA 10-K excerpt (customer concentration)",
    ticker: "NVDA",
    kind: "filing",
    period: "FY2025",
    content:
      "Data center revenue was 47.5 billion dollars for the quarter. " +
      "Two direct customers accounted for 24 percent of total revenue. " +
      "A loss of one large customer would materially reduce data center revenue.",
  },
  {
    sourceId: "nvda-call-q4fy2025",
    title: "NVDA earnings call (supply)",
    ticker: "NVDA",
    kind: "call",
    period: "Q4FY2025",
    content:
      "Supply of the newest accelerator remains constrained through the first half. " +
      "Lead times improved from 11 months to 8 months. " +
      "Gross margin is expected to stay in the mid 70s percent range.",
  },
  {
    sourceId: "desk-note-concentration",
    title: "Desk risk note (hyperscaler concentration)",
    ticker: "NVDA",
    kind: "note",
    period: "2025",
    content:
      "Concentration risk: a small number of hyperscale buyers drive most accelerator demand. " +
      "If two of them cut orders in the same quarter, the revenue shortfall is not replaceable within the year.",
  },
];

export const HEADLINES = [
  { ticker: "NVDA", headline: "Hyperscaler trims accelerator orders for next quarter" },
  { ticker: "AAPL", headline: "Services revenue hits a record and beats consensus" },
];
