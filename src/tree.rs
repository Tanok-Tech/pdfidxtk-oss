//! `pitk optimize` — optimización del árbol estilo PageIndex.
//!
//! PageIndex fusiona/nodes para minimizar el *worst-case search cost*: cuántas
//! páginas debe abrir el agente para alcanzar cualquier contenido. Aquí:
//! hermanos consecutivos cuya unión de páginas no supera `max_pages` se
//! fusionan en un nodo (bottom-up). El árbol crudo se conserva en
//! `tree.raw.json` para poder re-optimizar con otros parámetros.
//!
//! `tree.json` = optimizado (lo lee el agente) · `tree.raw.json` = original.

use serde::{Deserialize, Serialize};

use crate::model::TreeFile;

pub const DEFAULT_MAX_PAGES: usize = 8;

/// Nodo del índice listo para mostrar (árbol recortado a `depth` niveles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlineNode {
    pub id: String,
    pub title: String,
    pub level: u8,
    pub page: [usize; 2],
    pub kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OutlineNode>,
}

/// Recorre `tree` desde la raíz y devuelve los nodos hasta `depth` niveles
/// por debajo de la raíz. `depth = 0` → solo la raíz.
pub fn outline(tree: &TreeFile, depth: u8) -> Vec<OutlineNode> {
    let Some(root) = tree.nodes.get(&tree.root) else {
        return Vec::new();
    };
    vec![walk(tree, root, 0, depth)]
}

fn walk(tree: &TreeFile, node: &crate::model::TreeNode, level: u8, depth: u8) -> OutlineNode {
    let mut children = Vec::new();
    if level < depth {
        for id in &node.children {
            if let Some(ch) = tree.nodes.get(id) {
                children.push(walk(tree, ch, level + 1, depth));
            }
        }
    }
    OutlineNode {
        id: node.id.clone(),
        title: node.title.clone(),
        level: node.level,
        page: node.page,
        kind: node.kind.clone(),
        children,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptStats {
    pub max_pages: usize,
    pub nodes_before: usize,
    pub nodes_after: usize,
    pub merged: usize,
    /// páginas cubiertas por el nodo más grande antes/después (costo de abrirlo)
    pub max_span_before: usize,
    pub max_span_after: usize,
}

fn span(tree: &TreeFile, id: &str) -> (usize, usize) {
    tree.nodes
        .get(id)
        .map(|n| (n.page[0], n.page[1]))
        .unwrap_or((0, 0))
}

fn span_len(from: (usize, usize)) -> usize {
    from.1.saturating_sub(from.0).saturating_add(1)
}

pub fn stats(tree: &TreeFile, max_pages: usize) -> OptStats {
    let nodes_before = tree.nodes.len();
    let max_span_before = tree.nodes.values().map(|n| span_len((n.page[0], n.page[1]))).max().unwrap_or(0);
    OptStats {
        max_pages,
        nodes_before,
        nodes_after: nodes_before,
        merged: 0,
        max_span_before,
        max_span_after: max_span_before,
    }
}

/// Devuelve una copia optimizada del árbol + estadísticas.
pub fn optimize(tree: &TreeFile, max_pages: usize) -> (TreeFile, OptStats) {
    let mut out = tree.clone();
    let mut st = stats(&out, max_pages);

    if max_pages >= 2 && !out.nodes.is_empty() {
        let root = out.root.clone();
        let merged = merge_children(&mut out, &root, max_pages);
        st.merged = merged;
    }

    st.nodes_after = out.nodes.len();
    st.max_span_after = out
        .nodes
        .values()
        .map(|n| span_len((n.page[0], n.page[1])))
        .max()
        .unwrap_or(0);

    (out, st)
}

/// Fusiona hermanos consecutivos (bottom-up). Devuelve cuántos nodos se fusionaron.
fn merge_children(tree: &mut TreeFile, node_id: &str, max_pages: usize) -> usize {
    let children: Vec<String> = match tree.nodes.get(node_id) {
        Some(n) => n.children.clone(),
        None => return 0,
    };
    if children.len() < 2 {
        return 0;
    }

    // primero optimizar hacia abajo
    let mut merged = 0usize;
    for c in &children {
        merged += merge_children(tree, c, max_pages);
    }

    // luego fusionar hermanos consecutivos cuya unión quepa en max_pages
    let mut kept: Vec<String> = Vec::new();
    let mut acc: Option<String> = None;
    for c in children {
        match acc.take() {
            None => acc = Some(c),
            Some(a) => {
                let (a_start, a_end) = span(tree, &a);
                let (c_start, c_end) = span(tree, &c);
                let union = (a_start.min(c_start), a_end.max(c_end));
                if span_len(union) <= max_pages {
                    // c entra dentro de a
                    let (c_children, c_blocks) = match tree.nodes.get(&c) {
                        Some(cn) => (cn.children.clone(), cn.blocks.clone()),
                        None => (Vec::new(), Vec::new()),
                    };
                    if let Some(an) = tree.nodes.get_mut(&a) {
                        an.page = [union.0, union.1];
                        an.children.extend(c_children);
                        an.blocks.extend(c_blocks);
                        // el título queda el del primero; el resto queda en children/blocks
                    }
                    tree.nodes.remove(&c);
                    merged += 1;
                    acc = Some(a);
                } else {
                    kept.push(a);
                    acc = Some(c);
                }
            }
        }
    }
    if let Some(a) = acc {
        kept.push(a);
    }

    if let Some(n) = tree.nodes.get_mut(node_id) {
        n.children = kept;
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TreeNode;

    fn node(id: &str, title: &str, page: [usize; 2], children: Vec<&str>) -> TreeNode {
        TreeNode {
            id: id.into(),
            title: title.into(),
            level: 1,
            page,
            kind: "content".into(),
            children: children.into_iter().map(|s| s.to_string()).collect(),
            blocks: Vec::new(),
            summary: None,
        }
    }

    fn sample_tree() -> TreeFile {
        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(
            "n0".to_string(),
            TreeNode {
                id: "n0".into(),
                title: "doc".into(),
                level: 0,
                page: [1, 40],
                kind: "section".into(),
                children: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                blocks: Vec::new(),
                summary: None,
            },
        );
        nodes.insert("a".into(), node("a", "uno", [1, 1], vec![]));
        nodes.insert("b".into(), node("b", "dos", [2, 3], vec!["b1"]));
        nodes.insert("b1".into(), node("b1", "dos-hijo", [3, 3], vec![]));
        nodes.insert("c".into(), node("c", "tres", [4, 5], vec![]));
        nodes.insert("d".into(), node("d", "diez", [6, 20], vec![]));
        TreeFile {
            schema: "t".into(),
            doc_id: "x".into(),
            title: "doc".into(),
            pages: 40,
            root: "n0".into(),
            nodes,
        }
    }

    #[test]
    fn fusiona_hermanos_que_caben() {
        let t = sample_tree();
        let (out, st) = optimize(&t, 8);
        // [1,1]+[2,3]+[4,5] → [1,5] (5 ≤ 8); [6,20] no cabe
        assert_eq!(st.nodes_before, 6, "n0,a,b,b1,c,d");
        assert_eq!(st.nodes_after, 4, "n0, a(fusionado), d, b1: {:?}", st);
        assert_eq!(st.merged, 2);
        let root = &out.nodes["n0"];
        assert_eq!(root.children.len(), 2);
        let first = &out.nodes[&root.children[0]];
        assert_eq!(first.page, [1, 5]);
        assert_eq!(first.title, "uno", "la identidad la conserva el primero");
        assert_eq!(first.children, vec!["b1".to_string()], "subárbol de b preservado");
        assert!(out.nodes.contains_key("b1"), "el nieto no se pierde");
        assert!(!out.nodes.contains_key("b") && !out.nodes.contains_key("c"));
        assert_eq!(st.max_span_before, 40, "la raíz cubre 1..40");
        assert_eq!(st.max_span_after, 40);
    }

    #[test]
    fn sin_fusion_si_no_cabe() {
        let mut t = sample_tree();
        t.nodes.remove("b");
        t.nodes.remove("b1");
        t.nodes.remove("c");
        t.nodes.remove("d");
        t.nodes.get_mut("n0").unwrap().children = vec!["a".into()];
        let (out, st) = optimize(&t, 8);
        assert_eq!(st.nodes_before, 2);
        assert_eq!(st.nodes_after, 2);
        assert_eq!(st.merged, 0);
        assert_eq!(out.nodes.len(), 2);
    }

    #[test]
    fn off_no_toca_nada() {
        let t = sample_tree();
        let (out, st) = optimize(&t, 0);
        assert_eq!(st.merged, 0);
        assert_eq!(out.nodes.len(), t.nodes.len());
    }
}
