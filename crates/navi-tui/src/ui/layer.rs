#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ZIndex(pub(crate) i16);

pub(crate) mod z {
    use super::ZIndex;

    pub(crate) const FLOATING: ZIndex = ZIndex(50);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayerItem<T> {
    pub(crate) z: ZIndex,
    order: usize,
    pub(crate) item: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LayerStack<T> {
    items: Vec<LayerItem<T>>,
    next_order: usize,
}

impl<T> Default for LayerStack<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            next_order: 0,
        }
    }
}

impl<T> LayerStack<T> {
    pub(crate) fn push(&mut self, z: ZIndex, item: T) {
        self.items.push(LayerItem {
            z,
            order: self.next_order,
            item,
        });
        self.next_order = self.next_order.saturating_add(1);
    }

    pub(crate) fn into_paint_order(mut self) -> Vec<LayerItem<T>> {
        self.items.sort_by_key(|layer| (layer.z, layer.order));
        self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_stack_paints_low_to_high_z() {
        let mut stack = LayerStack::default();
        stack.push(ZIndex(10), "middle");
        stack.push(ZIndex(1), "bottom");
        stack.push(ZIndex(20), "top");

        let labels = stack
            .into_paint_order()
            .into_iter()
            .map(|layer| layer.item)
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["bottom", "middle", "top"]);
    }

    #[test]
    fn layer_stack_preserves_order_with_same_z() {
        let mut stack = LayerStack::default();
        stack.push(ZIndex(10), "first");
        stack.push(ZIndex(10), "second");

        let labels = stack
            .into_paint_order()
            .into_iter()
            .map(|layer| layer.item)
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["first", "second"]);
    }
}
