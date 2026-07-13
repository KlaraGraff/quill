import type {
  LearningCardKind,
  LearningCardNote,
  LearningCardResult,
  LearningModuleContent,
  LearningModuleId,
} from "./types";

type FixtureLanguage = "en" | "zh";

const content = (
  summary: string,
  details: string[] = [],
  items: LearningModuleContent["items"] = [],
  meta: string[] = [],
): LearningModuleContent => ({ summary, details, items, meta });

const wordModules = (language: FixtureLanguage): Partial<Record<LearningModuleId, LearningModuleContent>> =>
  language === "zh" ? {
    context_meaning: content(
      "这里指两个系统、群体或领域相互接触并产生作用的边界。",
      ["作者强调的不是静态边界，而是双方交换信息并彼此影响的位置。", "语气中性，常用于技术或抽象讨论。"],
    ),
    word_info: content("interfaces", ["原形：interface", "常见复数形式：interfaces"], [], ["/ˈɪntəfeɪsɪz/", "noun", "plural"]),
    target_translation: content("界面；交界处；接口"),
    common_senses: content("常见含义", [], [
      { title: "交界处", text: "两个系统或领域接触并互动的位置。", examples: [{ source: "The teams work at the interface of design and engineering.", target: "这些团队在设计与工程的交界处协作。" }] },
      { title: "接口", text: "让人或系统进行操作、连接的方式。" },
    ]),
    collocations: content("常见搭配", [], [
      { title: "user interface", text: "用户界面" },
      { title: "interface with", text: "与……连接或协作" },
    ]),
    morphology: content("由 inter-（在……之间）和 face（表面）构成。", ["interface 既可作名词，也可作动词。", "派生表达：interface design、interfacing。"]),
    grammar_role: content("在句中作复数名词。", ["它是介词 at 的宾语，并由形容词 digital 修饰。"]),
    synonyms: content("boundary 强调界线；connection 强调连接；interface 强调发生互动的接触面。"),
    usage: content("常见于技术、设计、组织协作和跨学科语境。"),
    memory_aid: content("把它理解为 inter（在两者之间）+ face（相接的面）。"),
    source_excerpt: { quote: "New ideas often emerge at the interfaces between established fields." },
  } : {
    context_meaning: content(
      "The points where two systems, groups, or fields meet and affect each other.",
      ["The author means an active place of exchange, not only a fixed boundary.", "The tone is neutral and slightly technical."],
    ),
    word_info: content("interfaces", ["Lemma: interface", "Common plural: interfaces"], [], ["/ˈɪntəfeɪsɪz/", "noun", "plural"]),
    target_translation: content("界面；交界处；接口"),
    common_senses: content("Common meanings", [], [
      { title: "A meeting point", text: "A place where two systems or fields interact.", examples: [{ source: "The teams work at the interface of design and engineering.", target: "这些团队在设计与工程的交界处协作。" }] },
      { title: "A control surface", text: "The way a person or system connects to a tool." },
    ]),
    collocations: content("Common combinations", [], [
      { title: "user interface", text: "The controls and screens used to operate software." },
      { title: "interface with", text: "To connect or work with something." },
    ]),
    morphology: content("Built from inter- (between) and face.", ["Interface can be a noun or a verb.", "Related forms include interface design and interfacing."]),
    grammar_role: content("A plural noun in this sentence.", ["It is the object of at and is modified by digital."]),
    synonyms: content("Boundary stresses a dividing line; connection stresses a link; interface stresses an interacting surface."),
    usage: content("Common in technology, design, organizations, and interdisciplinary work."),
    memory_aid: content("Think inter (between) + face (a surface that meets another surface)."),
    source_excerpt: { quote: "New ideas often emerge at the interfaces between established fields." },
  };

const phraseModules = (language: FixtureLanguage): Partial<Record<LearningModuleId, LearningModuleContent>> =>
  language === "zh" ? {
    context_meaning: content("这里表示某件事最终反而带来了积极、意外的结果。", ["说话者是在回顾一个起初看似不利的变化。"]),
    target_translation: content("结果证明这是因祸得福。"),
    common_senses: content("常用于描述坏事带来未预料到的好处。", [], [
      { title: "a blessing in disguise", text: "表面是坏事，后来才发现有好处。", examples: [{ source: "Missing that train was a blessing in disguise.", target: "没赶上那班火车反而是因祸得福。" }] },
    ]),
    collocations: content("常与 turn out to be、prove to be 搭配。"),
    grammar_analysis: content("整个短语在句中作表语。", ["in disguise 是介词短语，修饰 blessing。"]),
    idioms: content("这是固定习语，不能按“伪装的祝福”逐字理解。"),
    usage: content("适合用于回顾已经显现积极结果的事件。"),
    source_excerpt: { quote: "Losing the contract turned out to be a blessing in disguise." },
  } : {
    context_meaning: content("Something that first looked harmful but later produced an unexpected benefit.", ["The speaker is looking back after the positive result became clear."]),
    target_translation: content("结果证明这是因祸得福。"),
    common_senses: content("Used when a bad event leads to an unforeseen advantage.", [], [
      { title: "a blessing in disguise", text: "An apparent problem that later proves helpful.", examples: [{ source: "Missing that train was a blessing in disguise.", target: "没赶上那班火车反而是因祸得福。" }] },
    ]),
    collocations: content("Often follows turn out to be or prove to be."),
    grammar_analysis: content("The full phrase is a subject complement.", ["In disguise is a prepositional phrase modifying blessing."]),
    idioms: content("This is a fixed idiom; its meaning is not the literal sum of the words."),
    usage: content("Use it when the positive outcome is already visible."),
    source_excerpt: { quote: "Losing the contract turned out to be a blessing in disguise." },
  };

const passageModules = (language: FixtureLanguage): Partial<Record<LearningModuleId, LearningModuleContent>> =>
  language === "zh" ? {
    context_meaning: content("作者认为，真正重要的发现往往出现在成熟学科相互接触的地方。", ["这句话承接上文对专业分工的讨论，并把重点转向跨领域合作。", "隐含观点是：过于封闭的知识边界会限制创新。"]),
    target_translation: content("新的想法往往产生于成熟领域之间的交界处。"),
    grammar_analysis: content("主干是 New ideas emerge。", ["often 是频率副词；at the interfaces 是地点状语；between established fields 修饰 interfaces。"]),
    key_terms: content("理解这句话的三个关键词", [], [
      { title: "emerge", text: "出现、逐渐显现。", examples: [{ source: "A clear pattern began to emerge.", target: "一个清晰的模式开始显现。" }] },
      { title: "interfaces", text: "发生互动的交界处。" },
      { title: "established", text: "已经成熟、得到认可的。" },
    ]),
    idioms: content("本句没有必须整体理解的习语。"),
    references: content("established fields 指上文讨论的传统学科。"),
    reusable_patterns: content("X often emerges at the interface between A and B.", ["可用于描述跨学科创新或两种方法结合后的结果。"]),
    tone: content("语气概括而肯定，带有鼓励跨领域合作的意味。"),
    source_excerpt: { quote: "New ideas often emerge at the interfaces between established fields." },
  } : {
    context_meaning: content("The author argues that important discoveries often happen where mature disciplines meet.", ["The sentence shifts the discussion from specialization to collaboration across fields.", "It implies that rigid knowledge boundaries can limit innovation."]),
    target_translation: content("新的想法往往产生于成熟领域之间的交界处。"),
    grammar_analysis: content("The main clause is New ideas emerge.", ["Often is an adverb of frequency; at the interfaces gives the place; between established fields modifies interfaces."]),
    key_terms: content("Three terms carry the meaning", [], [
      { title: "emerge", text: "To appear or gradually become clear.", examples: [{ source: "A clear pattern began to emerge.", target: "一个清晰的模式开始显现。" }] },
      { title: "interfaces", text: "Places where different things interact." },
      { title: "established", text: "Already recognized and well developed." },
    ]),
    idioms: content("There is no fixed idiom that must be read as one unit."),
    references: content("Established fields refers to the traditional disciplines discussed earlier."),
    reusable_patterns: content("X often emerges at the interface between A and B.", ["Useful for describing interdisciplinary work or combined methods."]),
    tone: content("The tone is confident and encourages work across disciplinary boundaries."),
    source_excerpt: { quote: "New ideas often emerge at the interfaces between established fields." },
  };

export function getLearningCardFixture(
  kind: LearningCardKind,
  language: string,
): LearningCardResult {
  const fixtureLanguage: FixtureLanguage = language === "zh" ? "zh" : "en";
  const sourceText = kind === "word"
    ? "interfaces"
    : kind === "phrase"
      ? "a blessing in disguise"
      : "New ideas often emerge at the interfaces between established fields.";
  const modules = kind === "word"
    ? wordModules(fixtureLanguage)
    : kind === "phrase"
      ? phraseModules(fixtureLanguage)
      : passageModules(fixtureLanguage);
  return { version: 1, kind, sourceText, modules };
}

export const LEARNING_CARD_NOTE_FIXTURE: LearningCardNote[] = [
  {
    id: "preview-note",
    content: "Compare this use with the more technical meaning in software design.",
    updatedAt: Date.UTC(2026, 6, 13, 10, 0),
    scope: "book",
  },
];
