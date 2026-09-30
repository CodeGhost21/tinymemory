//! The bundled parity corpus.
//!
//! Short personal notes of the kind an assistant is asked to remember, each
//! with one question that asks for it in other words. The questions avoid the
//! notes' distinctive terms where a person would ("dental visit" for a
//! "dentist appointment", "plane" for "flight"), so an engine that only
//! matches words scores visibly lower than one that follows meaning. All names
//! and details are invented.

/// One note and the question that should find it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParityNote {
    /// The note's key, unique within the corpus.
    pub key: &'static str,
    /// What is stored.
    pub note: &'static str,
    /// What is asked; the note is its one correct answer.
    pub question: &'static str,
}

const fn note(key: &'static str, note: &'static str, question: &'static str) -> ParityNote {
    ParityNote {
        key,
        note,
        question,
    }
}

/// Thirty notes and their questions.
pub const BUNDLED_CORPUS: [ParityNote; 30] = [
    note(
        "coffee-order",
        "Priya takes her coffee as a flat white with oat milk and no sugar.",
        "How does Priya like her coffee?",
    ),
    note(
        "sister-birthday",
        "My sister Meera's birthday is on the 14th of March.",
        "When should I wish Meera a happy birthday?",
    ),
    note(
        "dentist",
        "The dentist appointment moved to Thursday at 4:30 pm.",
        "When is my dental visit?",
    ),
    note(
        "car",
        "I drive a blue 2019 Honda Jazz.",
        "What vehicle do I own?",
    ),
    note(
        "allergy",
        "Arjun is allergic to peanuts and shellfish.",
        "Which foods make Arjun sick?",
    ),
    note(
        "gym",
        "Gym sessions are on Monday, Wednesday and Friday at 7 am.",
        "Which days do I work out?",
    ),
    note(
        "project-deadline",
        "The Atlas migration must ship before the end of October.",
        "By when does the Atlas move have to be finished?",
    ),
    note(
        "manager",
        "Steven is my manager; our one-on-ones are on Tuesdays.",
        "Who do I report to at work?",
    ),
    note(
        "flight",
        "Flight AI-504 to Bengaluru departs at 06:15 from Terminal 2.",
        "What time does my plane leave?",
    ),
    note(
        "plant",
        "Water the fiddle-leaf fig every ten days; it hates cold drafts.",
        "How often does the houseplant need watering?",
    ),
    note(
        "book",
        "I am currently reading The Dispossessed by Ursula K. Le Guin.",
        "Which novel am I in the middle of?",
    ),
    note(
        "language",
        "I am learning Portuguese with a tutor on Saturday mornings.",
        "What language am I studying?",
    ),
    note(
        "rent",
        "Rent of 32,000 rupees is due on the 5th of each month.",
        "When do I have to pay the landlord?",
    ),
    note(
        "vitamin",
        "Dr. Kapoor prescribed vitamin D, one capsule every week.",
        "What supplement did the doctor put me on?",
    ),
    note(
        "editor",
        "For coding I use Neovim with the Catppuccin theme.",
        "Which text editor do I program in?",
    ),
    note(
        "parking",
        "The car is parked on level B2, spot 47, near the lifts.",
        "Where did I leave the car in the garage?",
    ),
    note(
        "anniversary",
        "Our wedding anniversary is on the 2nd of December.",
        "When is the day we got married celebrated?",
    ),
    note(
        "pet-food",
        "Mochi the cat eats salmon kibble twice a day.",
        "What does Mochi get fed?",
    ),
    note(
        "hotel",
        "For the Goa trip I booked the Seaview Inn, checking in on the 18th.",
        "Where am I staying on the beach holiday?",
    ),
    note(
        "shoe-size",
        "My shoe size is UK 9, wide fit.",
        "What size footwear should I buy?",
    ),
    note(
        "standup",
        "The team standup moves to 10:15 am starting next week.",
        "What time is the daily team sync now?",
    ),
    note(
        "podcast",
        "On long drives I listen to history podcasts.",
        "What do I play in the car on road trips?",
    ),
    note(
        "blood-type",
        "My blood group is O positive.",
        "What is my blood type?",
    ),
    note(
        "mentor",
        "Rhea mentors me on system design every other Friday.",
        "Who coaches me on software architecture?",
    ),
    note(
        "tea",
        "Evening tea is masala chai with ginger and less sugar.",
        "How do I take my chai?",
    ),
    note(
        "laptop",
        "My work laptop is a 14-inch MacBook Pro with 16 GB of memory.",
        "What computer do I use at the office?",
    ),
    note(
        "insurance",
        "Health insurance renews on the 1st of April.",
        "When does my medical cover need renewing?",
    ),
    note(
        "spare-key",
        "Our neighbour Mr. Iyer keeps the spare house key.",
        "Who has the extra key to the house?",
    ),
    note(
        "race",
        "I am training for the Mumbai half marathon in January.",
        "Which running event am I preparing for?",
    ),
    note(
        "wifi-router",
        "The home Wi-Fi router is in the hallway cupboard.",
        "Where is the internet box at home?",
    ),
];
