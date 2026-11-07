// A map in insertion order (Rust's IndexMap, Python's dict) for keys with std::hash.
#pragma once

#include <cstddef>
#include <unordered_map>
#include <utility>
#include <vector>

namespace vitalstream::core {

template <class K, class V>
class OrderedMap {
 public:
  using value_type = std::pair<K, V>;

  std::size_t size() const { return items_.size(); }
  bool empty() const { return items_.empty(); }
  bool contains(const K& k) const { return index_.count(k) != 0; }

  const V* get(const K& k) const {
    auto it = index_.find(k);
    return it == index_.end() ? nullptr : &items_[it->second].second;
  }
  V* get(const K& k) {
    auto it = index_.find(k);
    return it == index_.end() ? nullptr : &items_[it->second].second;
  }
  // The value of k, inserted (at the end) as `init` when absent.
  V& entry(const K& k, V init = V()) {
    auto it = index_.find(k);
    if (it != index_.end()) return items_[it->second].second;
    index_.emplace(k, items_.size());
    items_.emplace_back(k, std::move(init));
    return items_.back().second;
  }
  // Insert or replace (an existing key keeps its position).
  void insert(const K& k, V v) { entry(k) = std::move(v); }
  // Remove k, keeping the order of the others (IndexMap::shift_remove).
  bool shift_remove(const K& k, V* out = nullptr) {
    auto it = index_.find(k);
    if (it == index_.end()) return false;
    std::size_t i = it->second;
    if (out) *out = std::move(items_[i].second);
    items_.erase(items_.begin() + static_cast<std::ptrdiff_t>(i));
    index_.erase(it);
    for (auto& [key, pos] : index_)
      if (pos > i) --pos;
    return true;
  }
  void clear() {
    items_.clear();
    index_.clear();
  }

  auto begin() const { return items_.begin(); }
  auto end() const { return items_.end(); }
  auto begin() { return items_.begin(); }
  auto end() { return items_.end(); }

 private:
  std::vector<value_type> items_;
  std::unordered_map<K, std::size_t> index_;
};

}  // namespace vitalstream::core
