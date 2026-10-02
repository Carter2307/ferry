export type GlossaryEntry = {
  /** As it shows in a sentence: lower case, unless it is a name. */
  term: string;
  /** One or two sentences, 30 words at most, simple words. `code` between backticks. */
  definition: string;
  /** Ids of related terms (from any file of this folder). */
  see?: string[];
};
