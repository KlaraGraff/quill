import {
  DndContext,
  DragOverlay,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import { restrictToVerticalAxis } from "@dnd-kit/modifiers";
import {
  SortableContext,
  arrayMove,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useState, type CSSProperties, type ReactNode, type SyntheticEvent } from "react";

interface SortableRenderState {
  dragging: boolean;
  overlay: boolean;
}

interface SortableListProps<T> {
  items: T[];
  getId: (item: T) => string;
  onReorder: (items: T[]) => void | Promise<void>;
  renderItem: (item: T, index: number, state: SortableRenderState) => ReactNode;
  disabled?: boolean | ((item: T) => boolean);
  className?: string;
}

const INTERACTIVE_SELECTOR = "input,textarea,select,a,[contenteditable='true'],[data-no-drag]";

function isInteractiveTarget(event: SyntheticEvent) {
  return (event.target as Element | null)?.closest(INTERACTIVE_SELECTOR) !== null;
}

function SortableItem<T>({
  item,
  index,
  id,
  disabled,
  renderItem,
}: {
  item: T;
  index: number;
  id: string;
  disabled: boolean;
  renderItem: SortableListProps<T>["renderItem"];
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
    isOver,
  } = useSortable({ id, disabled });
  const style: CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 1 : undefined,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      onPointerDown={(event) => {
        if (!isInteractiveTarget(event)) listeners?.onPointerDown?.(event);
      }}
      onKeyDown={(event) => {
        if (!(event.target as Element | null)?.closest("button,input,textarea,select,a,[contenteditable='true'],[data-no-drag]")) {
          listeners?.onKeyDown?.(event);
        }
      }}
      className={`relative outline-none ${disabled ? "" : "cursor-grab active:cursor-grabbing"} ${isDragging ? "opacity-35" : "opacity-100"}`}
    >
      {isOver && !isDragging && (
        <span className="pointer-events-none absolute inset-x-0 -top-px z-10 h-0.5 bg-accent" />
      )}
      {renderItem(item, index, { dragging: isDragging, overlay: false })}
    </div>
  );
}

export default function SortableList<T>({
  items,
  getId,
  onReorder,
  renderItem,
  disabled = false,
  className,
}: SortableListProps<T>) {
  const [activeId, setActiveId] = useState<string | null>(null);
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );
  const ids = items.map(getId);
  const activeIndex = activeId ? ids.indexOf(activeId) : -1;
  const activeItem = activeIndex >= 0 ? items[activeIndex] : null;
  const itemDisabled = (item: T) => typeof disabled === "function" ? disabled(item) : disabled;

  const finishDrag = (event: DragEndEvent) => {
    setActiveId(null);
    if (!event.over || event.active.id === event.over.id) return;
    const from = ids.indexOf(String(event.active.id));
    const to = ids.indexOf(String(event.over.id));
    if (from < 0 || to < 0) return;
    void onReorder(arrayMove(items, from, to));
  };

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      modifiers={[restrictToVerticalAxis]}
      autoScroll
      onDragStart={({ active }) => setActiveId(String(active.id))}
      onDragCancel={() => setActiveId(null)}
      onDragEnd={finishDrag}
    >
      <SortableContext items={ids} strategy={verticalListSortingStrategy}>
        <div className={className}>
          {items.map((item, index) => (
            <SortableItem
              key={getId(item)}
              item={item}
              index={index}
              id={getId(item)}
              disabled={itemDisabled(item)}
              renderItem={renderItem}
            />
          ))}
        </div>
      </SortableContext>
      <DragOverlay dropAnimation={{ duration: 160, easing: "ease" }}>
        {activeItem ? (
          <div className="cursor-grabbing overflow-hidden rounded-md bg-bg-surface opacity-95 shadow-context">
            {renderItem(activeItem, activeIndex, { dragging: true, overlay: true })}
          </div>
        ) : null}
      </DragOverlay>
    </DndContext>
  );
}
