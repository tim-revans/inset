# Inset diagrams

What the generated diagrams show, and how a reader chooses what to see. It is a draft: the diagram generator does not exist yet. The sketches in each example's README show the intended pictures.

## Diagram data and views

The compiler emits diagram data that holds everything the source says about the program's structure. A view is drawn from that data, and only the view chooses what to show.

- Diagram data is complete and deterministic. It never leaves anything out to make a picture simpler.
- `inset diagram` draws a view as SVG. Its options choose what the view shows. With no options it shows everything below.
- An interactive viewer offers the same choices as toggles, and draws from the same data. A static picture and an interactive one with the same choices are the same picture.
- Hiding something never changes the layout's meaning: a hidden label is left off, and nothing is merged or moved to stand for it.

*Why the data holds everything and views choose:* most reviewers will use an interactive viewer, and different reviews need different detail. Someone checking what a program can touch wants every capability's type. Someone checking the flow of requests wants only names. If the generator decided, one of them would be shown less than the source says. Keeping every fact in the data means a view can only leave things out, never get them wrong.

## Labels

| Element | Shown by default | Can be hidden |
| --- | --- | --- |
| Capability edge | The field's name and its type: `items: Table<String, Item>` | The name or the type |
| Grant, in the app box | The whole declaration, with where each value comes from: `grant items: Table<String, Item> (fixed: stock.db)` | The type, the values |
| External end of a capability edge | What is outside: `( stock.db )`, `( HTTP clients )` | No |
| Message arrow | The message and its boundary notation: `ship(&sku, quantity)` | The parameters |
| Outlet | A port on the component's box, with its name and type: `low_stock: Outlet<Reorder>` | The type |
| Doc comments | As the description of the element they document | Yes |
| Failure section | Each arm and its reason | The reasons, or the whole section |

*Why the default shows both name and type:* the name is what the code and the requirements use, as in `given items[...]`, so the reviewer can match a step to an edge. The type says what kind of access it is. A reviewer who wants a lighter picture can hide either.

*(Draft: stage 3 fixes the option names and the full list of elements, and writes the rest of this page: the diagram style, and what it borrows from C4, UML activity diagrams and hardware schematics.)*
