// Harbor's support corpus. Short on purpose: every fact is checkable, so a
// cited answer can be verified by reading the source it points at.
// `sourceId` is the desk's own key. It goes in metadata so re-ingest is a no-op.

export const CORPUS = [
  {
    sourceId: "policy-refunds",
    title: "Refund policy",
    dept: "billing",
    content:
      "Harbor issues refunds within 14 days of purchase for unused items in original packaging. " +
      "Digital goods are refundable within 48 hours if they have not been downloaded. " +
      "Refunds are posted to the original payment method and take 5 to 7 business days to appear. " +
      "A restocking fee of 10 percent applies after 14 days and before 30 days. After 30 days, sales are final.",
  },
  {
    sourceId: "policy-shipping",
    title: "Shipping and delivery",
    dept: "shipping",
    content:
      "Standard shipping is 3 to 5 business days inside the US. Express shipping is 1 to 2 business days. " +
      "Orders placed after 3pm Eastern ship the next business day. " +
      "International orders clear customs in 5 to 12 days and are not eligible for express. " +
      "Lost packages are replaced after 10 business days from the original ship date.",
  },
  {
    sourceId: "policy-account",
    title: "Account and password",
    dept: "account",
    content:
      "Password resets are sent to the email on the account and expire in 30 minutes. " +
      "Two-factor authentication can be turned on from Settings, Security. " +
      "To close an account, the customer must have no open orders and a zero balance. " +
      "Closed accounts retain order history for 7 years for tax records.",
  },
  {
    sourceId: "policy-billing",
    title: "Billing and invoices",
    dept: "billing",
    content:
      "Harbor bills on the first of each month for subscriptions. Failed payments retry on day 3 and day 7. " +
      "After two failed retries the subscription pauses; data is kept for 30 days. " +
      "Invoices are available as PDF from Billing, Invoices. VAT is added for EU customers. " +
      "Chargebacks must be reported within 60 days of the statement date.",
  },
  {
    sourceId: "policy-warranty",
    title: "Hardware warranty",
    dept: "shipping",
    content:
      "Physical Harbor devices carry a 12 month limited warranty against manufacturing defects. " +
      "Water damage, drops, and unauthorized repair void the warranty. " +
      "Warranty replacements ship within 3 business days of approval. " +
      "The customer pays return shipping unless the unit failed in the first 30 days.",
  },
  {
    sourceId: "playbook-tone",
    title: "Support tone playbook",
    dept: "account",
    content:
      "Agents answer in two or three short sentences, name the policy, and offer the next step. " +
      "Do not invent SLAs that are not in a policy document. " +
      "If the question is about a refund window, cite the 14 day unused-item rule. " +
      "If the customer is angry, acknowledge the delay, then state the fact.",
  },
];

export const SAMPLE_TICKETS = [
  {
    subject: "Where is my order?",
    body: "I ordered on Monday and still do not have a tracking number. When will it ship?",
  },
  {
    subject: "Need a refund",
    body: "The headphones are unused and still in the box. I bought them 10 days ago. Can I get my money back?",
  },
  {
    subject: "Cannot log in",
    body: "The password reset email never arrives. I need to get into my account today.",
  },
];
