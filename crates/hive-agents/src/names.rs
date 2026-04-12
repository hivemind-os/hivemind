use uuid::Uuid;

const ADJECTIVES: &[&str] = &[
    "admiring",
    "agitated",
    "amazing",
    "angry",
    "awesome",
    "blissful",
    "bold",
    "boring",
    "brave",
    "busy",
    "charming",
    "clever",
    "cool",
    "cranky",
    "crazy",
    "dazzling",
    "determined",
    "distracted",
    "dreamy",
    "eager",
    "ecstatic",
    "elastic",
    "elated",
    "elegant",
    "epic",
    "excited",
    "fervent",
    "festive",
    "flamboyant",
    "focused",
    "friendly",
    "frosty",
    "funny",
    "gallant",
    "gifted",
    "goofy",
    "gracious",
    "happy",
    "hardcore",
    "heuristic",
    "hopeful",
    "hungry",
    "infallible",
    "inspiring",
    "intelligent",
    "interesting",
    "jolly",
    "jovial",
    "keen",
    "kind",
    "laughing",
    "loving",
    "lucid",
    "magical",
    "modest",
    "musing",
    "mystifying",
    "naughty",
    "nervous",
    "nice",
    "nifty",
    "nostalgic",
    "objective",
    "optimistic",
    "peaceful",
    "pedantic",
    "pensive",
    "practical",
    "priceless",
    "quirky",
    "quizzical",
    "recursing",
    "relaxed",
    "reverent",
    "romantic",
    "sad",
    "serene",
    "sharp",
    "silly",
    "sleepy",
    "stoic",
    "strange",
    "stupefied",
    "suspicious",
    "sweet",
    "tender",
    "thirsty",
    "trusting",
    "unruffled",
    "upbeat",
    "vibrant",
    "vigilant",
    "vigorous",
    "wizardly",
    "wonderful",
    "xenodochial",
    "youthful",
    "zealous",
    "zen",
];

const SCIENTISTS_AND_HACKERS: &[&str] = &[
    "archimedes",
    "babbage",
    "bell",
    "bohr",
    "brahmagupta",
    "brown",
    "carson",
    "chandrasekhar",
    "colden",
    "copernicus",
    "curie",
    "darwin",
    "davinci",
    "dijkstra",
    "einstein",
    "elion",
    "euler",
    "faraday",
    "fermat",
    "fermi",
    "feynman",
    "franklin",
    "galileo",
    "gauss",
    "goldberg",
    "goodall",
    "hawking",
    "heisenberg",
    "hodgkin",
    "hopper",
    "hugle",
    "hypatia",
    "jang",
    "joliot",
    "kare",
    "keller",
    "kepler",
    "khorana",
    "kilby",
    "knuth",
    "lalande",
    "lamarr",
    "leakey",
    "leavitt",
    "lichterman",
    "lovelace",
    "lumiere",
    "margulis",
    "maxwell",
    "mccarthy",
    "mcclintock",
    "meitner",
    "mendel",
    "mendeleev",
    "mirzakhani",
    "montalcini",
    "morse",
    "newton",
    "nightingale",
    "nobel",
    "noether",
    "northcutt",
    "panini",
    "pare",
    "pascal",
    "pasteur",
    "payne",
    "perlman",
    "pike",
    "planck",
    "ptolemy",
    "ramanujan",
    "ride",
    "ritchie",
    "roentgen",
    "rosalind",
    "rubin",
    "saha",
    "sammet",
    "sanderson",
    "satoshi",
    "shamir",
    "shannon",
    "snyder",
    "solomon",
    "spence",
    "stallman",
    "stonebraker",
    "swanson",
    "swirles",
    "tesla",
    "thompson",
    "torvalds",
    "turing",
    "varahamihira",
    "villani",
    "volhard",
    "wescoff",
    "wilbur",
    "williams",
    "wilson",
    "wozniak",
    "wu",
    "yalow",
    "yonath",
    "mitnick",
    "bernstein",
    "gosling",
    "kernighan",
    "mcilroy",
    "kay",
    "engelbart",
    "cerf",
    "berners_lee",
    "carmack",
    "schneier",
    "zimmermann",
    "rivest",
    "diffie",
    "hellman",
    "backus",
    "liskov",
    "hoare",
    "milner",
    "stroustrup",
    "wall",
    "matsumoto",
    "hickey",
    "odersky",
    "hejlsberg",
];

/// Generate a Docker-style friendly name in the format `adjective_name`.
pub fn generate_friendly_name() -> String {
    let bytes = Uuid::new_v4().into_bytes();
    let adj_idx = u16::from_le_bytes([bytes[0], bytes[1]]) as usize % ADJECTIVES.len();
    let name_idx = u16::from_le_bytes([bytes[2], bytes[3]]) as usize % SCIENTISTS_AND_HACKERS.len();
    format!("{}_{}", ADJECTIVES[adj_idx], SCIENTISTS_AND_HACKERS[name_idx])
}

/// Person-role emoji from the Unicode Full Emoji List (rows ~291–351).
/// Each entry is (base_person_char, optional ZWJ suffix).  Skin-tone
/// modifiers are inserted between the base and the suffix.
const PERSON_ROLE_EMOJI: &[(char, &str)] = &[
    // ZWJ profession sequences  (🧑 + ZWJ + profession symbol)
    ('\u{1F9D1}', "\u{200D}\u{2695}\u{FE0F}"), // 🧑‍⚕️  health worker
    ('\u{1F9D1}', "\u{200D}\u{1F393}"),        // 🧑‍🎓  student
    ('\u{1F9D1}', "\u{200D}\u{1F3EB}"),        // 🧑‍🏫  teacher
    ('\u{1F9D1}', "\u{200D}\u{2696}\u{FE0F}"), // 🧑‍⚖️  judge
    ('\u{1F9D1}', "\u{200D}\u{1F33E}"),        // 🧑‍🌾  farmer
    ('\u{1F9D1}', "\u{200D}\u{1F373}"),        // 🧑‍🍳  cook
    ('\u{1F9D1}', "\u{200D}\u{1F527}"),        // 🧑‍🔧  mechanic
    ('\u{1F9D1}', "\u{200D}\u{1F3ED}"),        // 🧑‍🏭  factory worker
    ('\u{1F9D1}', "\u{200D}\u{1F4BC}"),        // 🧑‍💼  office worker
    ('\u{1F9D1}', "\u{200D}\u{1F52C}"),        // 🧑‍🔬  scientist
    ('\u{1F9D1}', "\u{200D}\u{1F4BB}"),        // 🧑‍💻  technologist
    ('\u{1F9D1}', "\u{200D}\u{1F3A4}"),        // 🧑‍🎤  singer
    ('\u{1F9D1}', "\u{200D}\u{1F3A8}"),        // 🧑‍🎨  artist
    ('\u{1F9D1}', "\u{200D}\u{2708}\u{FE0F}"), // 🧑‍✈️  pilot
    ('\u{1F9D1}', "\u{200D}\u{1F680}"),        // 🧑‍🚀  astronaut
    ('\u{1F9D1}', "\u{200D}\u{1F692}"),        // 🧑‍🚒  firefighter
    // Standalone person-role emoji  (single codepoint + skin tone)
    ('\u{1F46E}', ""), // 👮  police officer
    ('\u{1F482}', ""), // 💂  guard
    ('\u{1F477}', ""), // 👷  construction worker
    ('\u{1F934}', ""), // 🤴  prince
    ('\u{1F478}', ""), // 👸  princess
    ('\u{1F473}', ""), // 👳  person wearing turban
    ('\u{1F935}', ""), // 🤵  person in tuxedo
    ('\u{1F470}', ""), // 👰  person with veil
    ('\u{1F47C}', ""), // 👼  baby angel
    ('\u{1F385}', ""), // 🎅  Santa Claus
    ('\u{1F936}', ""), // 🤶  Mrs. Claus
    ('\u{1F9B8}', ""), // 🦸  superhero
    ('\u{1F9B9}', ""), // 🦹  supervillain
    ('\u{1F9D9}', ""), // 🧙  mage
    ('\u{1F9DA}', ""), // 🧚  fairy
    ('\u{1F9DB}', ""), // 🧛  vampire
    ('\u{1F9DC}', ""), // 🧜  merperson
    ('\u{1F9DD}', ""), // 🧝  elf
    ('\u{1F977}', ""), // 🥷  ninja
];

const SKIN_TONES: &[char] = &[
    '\u{1F3FB}', // light
    '\u{1F3FC}', // medium-light
    '\u{1F3FD}', // medium
    '\u{1F3FE}', // medium-dark
    '\u{1F3FF}', // dark
];

/// Pick a random person-role emoji with a random skin tone.
pub fn generate_random_avatar() -> String {
    let bytes = Uuid::new_v4().into_bytes();
    let emoji_idx = bytes[0] as usize % PERSON_ROLE_EMOJI.len();
    let tone_idx = bytes[1] as usize % SKIN_TONES.len();
    let (base, suffix) = PERSON_ROLE_EMOJI[emoji_idx];
    let mut s = String::with_capacity(16);
    s.push(base);
    s.push(SKIN_TONES[tone_idx]);
    s.push_str(suffix);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friendly_name_has_underscore() {
        let name = generate_friendly_name();
        assert!(name.contains('_'), "Expected underscore in '{name}'");
    }

    #[test]
    fn friendly_names_are_varied() {
        let names: Vec<String> = (0..50).map(|_| generate_friendly_name()).collect();
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert!(unique.len() > 40, "Expected mostly unique names, got {} / 50", unique.len());
    }
}
